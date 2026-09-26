//! Prompt watcher loop: event stream + 2s poll tick, stream
//! accumulation, stall watch, and settle→finalize on completion. Split
//! from `finalize`/`settle` (300-line file limit). One task per prompt;
//! supersede/cancel retire via epoch + cancel signal, never by killing.
use crate::jobs::episode::BuzzEpisode;
use crate::jobs::runner_cancel::cancel_watch;
use crate::jobs::stall::watch_stall;
use crate::{
    herdr::client::get_agent,
    jobs::job::Job,
    jobs::settle::{SettleStep, SettledArm, settle_step, sleep_or_superseded},
    jobs::stream::WatchEvent,
    state::AppState,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::time::{Duration, Instant};

/// Terminal statuses that end a watch cycle.
const SETTLED: &[&str] = &["idle", "done", "blocked", "exited", "closed", "dead"];
/// Poll-failure backoff seed in case herdr events are unavailable.
const RETRY_BACKOFF_SECS: u64 = 5;
/// Min gap between event-socket reconnect attempts (prevents tight-loop starvation).
const REOPEN_COOLDOWN_SECS: u64 = 5;

/// Watch the agent via herdr push-events: output bursts accumulate for
/// the final-card arbitration; settle posts the final result card.
pub(crate) async fn watch_job(s: AppState, pane: String, job: Arc<Job>) {
    // Raw output since the prompt — the fresh reply is extracted from
    // this at display time (last segment only, see segment::final_block)
    let mut acc: Vec<String> = Vec::new();
    let mut ev = None;
    // checked_sub: Instant::now() - cooldown panics when the monotonic
    // clock is younger than the cooldown (fresh boot); fall back to now
    // (first reconnect delayed one cooldown, never a panic).
    let mut last_open = Instant::now()
        .checked_sub(Duration::from_secs(REOPEN_COOLDOWN_SECS))
        .unwrap_or_else(Instant::now);
    // Rate-limit episode already buzzed about (kind, not excerpt: retry
    // countdowns change every second and must not re-alert). Cleared
    // when the banner leaves the screen so the next episode re-alerts.
    // Provider fatals buzz only when stuck (see episode.rs): transients
    // recover into the final reply, terminal errors surface at settle.
    let mut episode = BuzzEpisode::new();
    let mut last_epoch = job.epoch.load(Ordering::Relaxed);
    let mut fails: u32 = 0;
    // Delivery-retry backoff: a dead Telegram must not spin herdr reads
    // at full tick rate forever — back off, keep the intent until
    // /cancel or pane death breaks the loop.
    let mut retry_wait = RETRY_BACKOFF_SECS;
    println!("[watcher] start {pane}");
    // Job-owned typing: fresh panes are event-blind until the next
    // resubscribe (≤120s) and short bursts fall between 60s watchdog
    // ticks, so observed transitions alone never start the indicator
    // for them. The watcher KNOWS the pane is being worked — sustain
    // typing here, stop at the end when no job remains. Skip when
    // already stopped (insert→cancel→spawn race) to avoid an orphan blip.
    if !job.is_stopped() {
        s.start_typing(&pane).await;
    }
    // Dedicated typing ticker (shared cadence, well inside the ≈5s
    // expiry, independent of herdr RPCs): thinking pauses with no
    // output/events go dark in DM mode without it (no typing task
    // there), and a slow get_agent would otherwise stretch the
    // piggyback period past expiry. Spawned, never awaited inline.
    let mut typing_tick =
        tokio::time::interval(Duration::from_secs(crate::state::TYPING_TICK_SECS));
    // Skip, never Burst: after slow RPC rounds a Burst catch-up would
    // fire ticks back-to-back, spinning tight poll cycles against an
    // already-sick herdr. Skipped ticks simply resume the 2s cadence.
    typing_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    typing_tick.tick().await;
    // Time-based sustain task: the ticker arm above falls through to
    // blocking work (30s agent reads, 45s stall scans), so sick-herdr
    // rounds would stretch the sustain gap past the ≈5s expiry — in DM
    // nothing else backstops it. Reads the current dest every round (a
    // remap retargets mid-watch); aborted once in the exit epilogue.
    let sustain = {
        let tg = s.tg.clone();
        let job = job.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(crate::state::TYPING_TICK_SECS)).await;
                if job.is_stopped() {
                    break;
                }
                let (c, t) = *job.dest.lock().await;
                tg.typing(c, t).await;
            }
        })
    };
    // Instant feedback is the typing indicator (sustained below on the
    // shared cadence, well inside the ≈5s expiry, so a returning client
    // sees it within ~2s) plus the silent progress message (placeholder
    // on submit, transient tail edited in place — only the final buzzes).
    // First settled sample arms the report timer (see settle.rs): agy
    // idles briefly between phases mid-run, and retiring on that
    // transient leaves the agent working unwatched. Cleared on working
    // samples, herdr errors, and epoch changes. Tracks the arming kind:
    // done→blocked→idle flips re-arm instead of committing as one
    // persistence.
    let mut settled_since: SettledArm = None;

    loop {
        if job.is_stopped() {
            // Quiet retire: no cancel card by design (the silent
            // transient retires in the exit footer), buzz nothing.
            break;
        }
        // New prompt on a reused watcher restarts all episode timers
        // and drops the old prompt's stream state: stale accumulation
        // and baseline belong to the previous turn.
        let epoch = job.epoch.load(Ordering::Relaxed);
        if epoch != last_epoch {
            last_epoch = epoch;
            episode.reset();
            settled_since = None;
            acc.clear();
            retry_wait = RETRY_BACKOFF_SECS;
            // Herdr-error streak belongs to the old prompt: 11 failures
            // there + 1 here must not back the new prompt off for 60s.
            fails = 0;
            // Baseline persists: it already covers everything streamed,
            // so the next delta is exactly the new turn's output
            // (resetting drops the first burst — fresh and reused).
        }
        // Reconnect the event stream lazily — never in a hot loop.
        // Bounded: a hung ack degrades to polling, never freezes pre-select.
        // Cancel-raced: the open await sits outside the select below, so
        // /cancel (or a supersede, via the epoch check next iteration)
        // must not wait behind up to 10s of dial.
        if ev.is_none() && last_open.elapsed() >= Duration::from_secs(REOPEN_COOLDOWN_SECS) {
            // Split to `runner_cancel::reopen_events` (300-line file limit).
            match super::runner_cancel::reopen_events(&s, &pane, &job, &mut ev, &mut last_open)
                .await
            {
                super::runner_cancel::Reopen::Break => break,
                // Epoch moved (or stopped): skip the dial entirely —
                // the loop-top epoch arm picks the new turn up.
                super::runner_cancel::Reopen::Cooled => continue,
                super::runner_cancel::Reopen::Ready => {}
            }
        }

        // Output activity → stream; status change → maybe finalize.
        // The typing tick doubles as the poll driver (2s cadence): every
        // wake-up runs a poll cycle below, so progress never depends on
        // herdr events (they only wake earlier). No separate fallback
        // sleep arm — a second timer would never fire ahead of the 2s
        // tick (recreated each iteration = dead code); slow-RPC catch-up
        // is Skip above, never Burst.
        // The typing arm fires on the shared cadence and falls through
        // to a poll cycle too (its own cost is one spawned `typing` RPC).
        let _event = tokio::select! {
            _ = job.cancel.notified() => {
                cancel_watch(&s, &pane, &job).await;
                break;
            }
            _ = typing_tick.tick() => {
                // Poll driver only now (the sustain task above owns the
                // indicator): instant feedback is typing plus the silent
                // progress message, sustained time-based.
                // Falls through to a poll cycle below (no `continue`).
                WatchEvent::Output
            }
            e = async {
                match ev.as_mut() {
                    Some(stream) => stream.next().await,
                    None => std::future::pending().await,
                }
            } => match e {
                Some(w) => w,
                // Stream death must still run a poll cycle below (a
                // settle landing exactly on death else waits a full extra
                // tick per flap) — clear and fall through as output.
                None => {
                    ev = None;
                    WatchEvent::Output
                }
            },
        };

        // Every wake-up: check settle first (never depend on herdr events),
        // then stream whatever output is new. (Dest typing lives on its
        // own ticker arm above, not piggybacked here.)
        let agent = match get_agent(&s.cfg.socket, &pane).await {
            Ok(a) => {
                fails = 0;
                a
            }
            Err(e) => {
                fails += 1;
                if fails >= 12 {
                    println!(
                        "[watcher] {pane} unreachable 12x in a row ({}) — backing off 60s, intent kept",
                        crate::types::mask_home(&e.to_string())
                    );
                    // Cancellable backoff: /cancel must not wait behind it,
                    // and a supersede (epoch bump, never notifies) must not
                    // stall the handoff for the full minute either.
                    // Same ownership rule as the cancel branch: a replaced
                    // watcher's clear must not eat the new job's intent.
                    let backoff_epoch = job.epoch.load(Ordering::Relaxed);
                    tokio::select! {
                        _ = job.cancel.notified() => {
                            cancel_watch(&s, &pane, &job).await;
                            break;
                        }
                        _ = sleep_or_superseded(&job, backoff_epoch, Duration::from_secs(60)) => {}
                    }
                    fails = 0;
                }
                // Unknown, not settled: an armed report timer must not
                // survive the outage (a post-outage sample would else
                // count as persistence spanning the whole blackout).
                settled_since = None;
                continue;
            }
        };
        if SETTLED.contains(&agent.status.as_str()) {
            match settle_step(
                &s,
                &pane,
                &job,
                &agent.status,
                &mut acc,
                &mut retry_wait,
                &mut settled_since,
                episode.error_stuck(std::time::Instant::now()),
                last_epoch,
            )
            .await
            {
                SettleStep::Continue => continue,
                SettleStep::Break => break,
            }
        }
        // Sampled working: any armed report timer was a transient.
        // Progress also decays the delivery-retry backoff: consecutive
        // outages escalate, healthy output in between resets.
        settled_since = None;
        retry_wait = RETRY_BACKOFF_SECS;

        // Rate-limit stall watch (see stall.rs): opencode retries
        // internally with no settle and no buzz — one NEW card per
        // episode, straight from the raw screen.
        let screen = watch_stall(&s, &pane, &job, &mut episode).await;
        // Streaming keeps the historic ~200-line window: the stall watch
        // reads wide (500) for quota detection; baselines and deltas stay
        // tail-shaped so finals behave exactly as before.
        let screen = if screen.len() > 200 {
            screen[screen.len() - 200..].to_vec()
        } else {
            screen
        };

        // Stream whatever is new into the accumulator (see live.rs
        // for the baseline/delta details), then reflect the raw tail
        // onto the silent instant message (transients included — the
        // final still arbitrates its clean reply as a NEW buzzing card).
        super::live::stream_live(&job, screen, &mut acc).await;
        super::progress::refresh_live(&s, &pane, &job, &acc).await;
    }

    // Every `break` above converges here: single abort site for the
    // sustain task (it also self-exits on stop as backstop).
    sustain.abort();
    // Ownerless exits (quiet pane-death retires post no card) must not
    // strand the silent placeholder beside nothing — a successor turn
    // (same-Arc epoch bump or another Arc in the map) keeps it.
    super::progress::clear_live_if_ownerless(&s, &pane, &job, last_epoch).await;
    // Retire only if the map still points at THIS watcher (no newer job took over)
    {
        let mut map = s.jobs.lock().await;
        // Epoch-gated like books::settle_books (same-Arc reuse bumps the
        // epoch in place — ptr_eq alone cannot tell a successor apart):
        // a submit landing between the last loop-top and this exit owns
        // the entry now; removing it strands a live watcher with no map
        // cover (a later failed submit reads owed==0 and retires it).
        if job.epoch.load(Ordering::Relaxed) == last_epoch
            && map
                .get(&pane)
                .map(|j| Arc::ptr_eq(j, &job))
                .unwrap_or(false)
        {
            map.remove(&pane);
        }
    }
    // Shared stop: only when nothing owns the pane (atomic — a
    // successor starting concurrently adopts or re-mints). Shell-aware:
    // an overlapping shell settle keeps its indicator.
    s.stop_shell_typing(&pane).await;
}
