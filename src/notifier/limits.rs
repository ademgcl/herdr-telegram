//! Watchdog limit-stall scanner: quota stalls never transition, so this
//! scans screens directly. Split from `reconcile` (300-line file limit);
//! pure predicates live in `limit_decide` (unit-tested there).
use crate::{
    herdr::client::read_screen_for_limits,
    jobs::notices::{detect_limit, limit_card_text, needs_stuck_gate},
    notifier::limit_claim::try_claim,
    notifier::limit_decide::{detect_tail_with_context, kind_flipped, scan_tail, send_cooled},
    state::AppState,
};
use std::time::{Duration, Instant};

/// Re-remind while a limit stall persists (shared with prompt watchers —
/// either path's alert suppresses the other per kind, so handoffs buzz once).
pub const LIMIT_REMIND_SECS: u64 = 1800;
// Stuck/clear hysteresis is single-sourced on the watcher episode
// (`jobs::episode::STUCK_SECS` / `CLEAR_MISSES`): the values below are
// aliases, never independent knobs — a one-sided edit silently desyncs
// the watcher↔watchdog handoff (double-page or swallowed refire).
use crate::jobs::episode::{CLEAR_MISSES as LIMIT_CLEAR_MISSES, STUCK_SECS as LIMIT_STUCK_SECS};

/// Buzz once per limit episode — plus a backstop for prompt-owned panes
/// whose watcher is parked (backoff), settling, or retired: stuck stalls
/// still page, so a dead watcher must not mean a silent user.
/// STRONG-`auth` banners alert on ANY non-shell status (herdr can sample
/// idle mid-retry) but only from the fresh tail (deep-scrollback
/// leftovers of a settled run stay silent); gated (`rate-limit`/
/// `provider`/`error`/WEAK-`auth`) ones still need a working pane.
/// Once-per-episode survives noise: empty/outage reads preserve all
/// state (unknown ≠ clean), a banner must be absent for
/// [`LIMIT_CLEAR_MISSES`] consecutive clean reads to clear the episode,
/// gated banners must persist [`LIMIT_STUCK_SECS`] before buzzing, kind
/// flips damp one tick, and a recent alert suppresses re-pages of the
/// SAME kind only (a provider blip must never hide later quota; the
/// 30-min remind still fires). Claims are atomic and delivery-gated: a
/// dropped send removes its claim and cools down instead of suppressing.
pub(crate) async fn scan_limits(s: &AppState) {
    // Snapshots only (never hold status/jobs across RPC): rows plus owned
    // watcher jobs; reply dests re-resolve from the live map at send time.
    let rows: Vec<(String, String)> = {
        s.status
            .lock()
            .await
            .iter()
            .map(|(p, st)| (p.clone(), st.clone()))
            .collect()
    };
    if rows.is_empty() {
        return;
    }
    for (pane, st) in rows {
        if st == "shell" {
            continue;
        }
        let screen = read_screen_for_limits(&s.cfg.socket, &pane).await;
        // Outage/unknown: preserve everything (alert, stuck timer, miss
        // count) so the next good read does NOT re-alert.
        if screen.is_empty() {
            continue;
        }
        // Settled panes count fresh-tail banners only: a quota banner in
        // deep scrollback under idle/done is a finished run's leftover.
        // Stale-only screens fall into the clean-miss branch and clear.
        // Context spans the full screen: a fresh WEAK banner with
        // error words only in scrollback is still a live stall.
        let full = detect_limit(&screen);
        let hit = if st == "working" {
            full
        } else {
            full.and_then(|_| detect_tail_with_context(scan_tail(&screen), &screen))
        };
        let Some(hit) = hit else {
            // Clean miss: only a sustained absence clears the episode.
            let misses = {
                let mut m = s.limit_miss.lock().await;
                let n = m.get(&pane).copied().unwrap_or(0) + 1;
                if n >= LIMIT_CLEAR_MISSES {
                    m.remove(&pane);
                } else {
                    m.insert(pane.clone(), n);
                }
                n
            };
            if misses >= LIMIT_CLEAR_MISSES {
                s.limit_alert.lock().await.remove(&pane);
                s.limit_seen.lock().await.remove(&pane);
                s.limit_send_cool.lock().await.remove(&pane);
            }
            continue;
        };
        // Banner present: absence streak over.
        s.limit_miss.lock().await.remove(&pane);
        let gated = needs_stuck_gate(&hit);
        // Gated stalls on settled panes never buzz (the settle/final card
        // owns them) — but the stuck timer still accumulates across idle
        // samples, so a return to working fires promptly instead of
        // restarting the 90s from zero. Owned panes are their watcher's
        // job while the job entry lives; once it retires they are job-less
        // and covered below like any other pane.
        if gated && st != "working" {
            let mut seen = s.limit_seen.lock().await;
            let same = seen.get(&pane).map(|(k, _)| k == hit.kind).unwrap_or(false);
            if !same {
                seen.insert(pane.clone(), (hit.kind.to_string(), Instant::now()));
            }
            continue;
        }
        // Stuck gate (gated) + flip damping (immediate) share limit_seen
        // (kind, first_seen): a flip restarts and skips one tick, so
        // co-present banners swapping the topmost line never spam, while
        // genuine transitions page on the next tick and transients only
        // after they persist. WEAK `auth` wording (agent prose, log dumps)
        // is gated too — only specific denials page at once.
        if gated {
            let stuck = {
                let mut seen = s.limit_seen.lock().await;
                match seen.get(&pane) {
                    Some((k, t)) if k == hit.kind => {
                        t.elapsed() >= Duration::from_secs(LIMIT_STUCK_SECS)
                    }
                    _ => {
                        seen.insert(pane.clone(), (hit.kind.to_string(), Instant::now()));
                        false
                    }
                }
            };
            if !stuck {
                continue;
            }
        } else {
            // Single guard (no TOCTOU with overlapping ticks): read the
            // previous kind and record the flip/first-sight atomically.
            let mut seen = s.limit_seen.lock().await;
            let prev = seen.get(&pane).map(|(k, _)| k.clone());
            let flipped = kind_flipped(prev.as_deref(), hit.kind);
            if flipped || prev.is_none() {
                seen.insert(pane.clone(), (hit.kind.to_string(), Instant::now()));
            }
            if flipped {
                continue;
            }
        }
        // Moved-on re-check (spontaneous/stall parity): a quit-to-shell
        // landing between the snapshot and the send must not page the
        // stale ❗ into the shelled topic. State preserves; next tick
        // re-evaluates.
        if s.status
            .lock()
            .await
            .get(&pane)
            .is_some_and(|v| v == "shell")
        {
            continue;
        }
        // Reply dest from the live map (prompt-owned panes report into
        // their own chat, exactly where their watcher would). No dest
        // anywhere ⇒ nothing to claim, nothing to send.
        let owner = {
            s.jobs
                .lock()
                .await
                .get(&pane)
                .cloned()
                .filter(|j| !j.is_stopped())
        };
        let mut owned_dest: Option<(i64, Option<i64>)> = None;
        if let Some(j) = &owner {
            owned_dest = Some(*j.dest.lock().await);
        }
        let thread = s.topics.all_mappings().get(&pane).copied();
        let forum_dest = s.cfg.forum.zip(thread);
        // Remap-safe (stall.rs parity): a paced reset migrates the topic
        // mid-run — buzzing the corpse thread fails or lands in the
        // deleted topic. Forum dests follow the live mapping; DM dests
        // never gain a thread.
        if let Some((chat, th)) = owned_dest.as_mut()
            && s.cfg.forum == Some(*chat)
            && let Some(cur) = thread
        {
            *th = Some(cur);
        }
        if owned_dest.is_none() && forum_dest.is_none() && s.cfg.owners.is_empty() {
            continue;
        }
        // Failed-send cooldown BEFORE claiming (preserves stuck timers and
        // damping records): a dead Telegram backs off instead of storming.
        let now = Instant::now();
        let cooled = s.limit_send_cool.lock().await.get(&pane).copied();
        if send_cooled(cooled, now) {
            continue;
        }
        // Atomic check-and-claim under one guard: concurrent watcher ticks
        // see the claim and stay silent (no double-page). Same-kind only —
        // a provider blip never hides later quota. Drop-guarded: the 50s
        // watchdog timeout drops this tick mid-send, and without the guard
        // the leaked claim would suppress the stall for 30 min undelivered.
        let Some(claim) = try_claim(s, &pane, hit.kind, now).await else {
            continue;
        };
        let text = limit_card_text(&pane, &hit);
        // All-or-nothing claim: partial delivery (some owners) still
        // retries next tick — a duplicate card to a healthy owner beats a
        // silent one for quota, and single-owner deployments (the norm)
        // never hit this branch partially. Dest fallback: a stale prompt
        // chat (pruned thread) falls back to the live topic; owners hear
        // it only when no forum exists at all (DM-mode deployments — in
        // forum mode an unmapped pane is a paced-reset remint in flight,
        // and DM-fallback would buzz the wrong surface instead of the new
        // topic; stall.rs parity is silence + retry next tick; forum sends
        // that fail transiently retry next tick instead of DM-spamming).
        let mut delivered = false;
        if let Some((chat, th)) = owned_dest {
            delivered = send_one(s, chat, th, &text, &pane).await;
        }
        if !delivered && let Some((fchat, fth)) = forum_dest {
            delivered = send_one(s, fchat, Some(fth), &text, &pane).await;
        }
        if !delivered && forum_dest.is_none() && s.cfg.forum.is_none() && !s.cfg.owners.is_empty() {
            // Per-owner (not product): one blocked owner must not hold the
            // healthy ones hostage for retries + every future tick, nor
            // spam the healthy with duplicates on retry. Complete when at
            // least one owner landed (spontaneous.rs dm_complete parity);
            // only none-landed releases the claim + cools down for retry.
            let mut per_owner = Vec::with_capacity(s.cfg.owners.len());
            for id in &s.cfg.owners {
                per_owner.push(usize::from(send_one(s, *id, None, &text, &pane).await));
            }
            delivered = crate::notifier::spontaneous::dm_complete(&per_owner, 1);
        }
        // Screen excerpts stay out of the log (terminal content can hold
        // secrets); kind + length are enough for stall forensics.
        if delivered {
            claim.keep();
            s.limit_send_cool.lock().await.remove(&pane);
            println!(
                "[alert] limit stall {pane}: {} ({} chars)",
                hit.kind,
                hit.excerpt.chars().count()
            );
        } else {
            // Release iff still ours (a concurrent success must survive)
            // and cool down: the next tick retries, nothing suppresses,
            // nothing storms.
            claim.release().await;
            s.limit_send_cool
                .lock()
                .await
                .insert(pane.clone(), Instant::now());
            eprintln!(
                "[alert] limit stall {pane}: {} send FAILED — cooling down",
                hit.kind
            );
        }
    }
}

/// One buzzing limit card + ❗ reaction; true only on delivery.
async fn send_one(s: &AppState, chat: i64, thread: Option<i64>, text: &str, pane: &str) -> bool {
    let mid =
        s.tg.send_msg_with_effect(chat, thread, text, None, Some(crate::telegram::EFFECT_FIRE))
            .await;
    if let Some(m) = mid {
        let _ = s.tg.set_reaction(chat, m, Some("❗")).await;
    }
    s.remember(chat, mid, pane).await;
    mid.is_some()
}
