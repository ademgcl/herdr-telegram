//! Dialog segmentation: split from `segment` (300-line file limit).
//! In blocked dialogs, '←' does not split headers and options are
//! preserved in raw form so terminal padding rules never drop them
//! before parsing.
use super::echo::is_input_opener;
use super::filter::dialog_chrome_filtered;
use super::segment::{is_dialog_boundary, is_footer_segment, is_rule};

/// Dialog-specific segmentation: returns (cleaned_winner, raw_winner).
/// In blocked dialogs, '←' does not split headers and options are preserved
/// in raw form so terminal padding rules never drop them before parsing.
pub fn dialog_block(lines: &[String]) -> (Vec<String>, Vec<String>) {
    let mut cands: Vec<(Vec<String>, Vec<String>, bool)> = Vec::new();
    let mut raw_seg: Vec<String> = Vec::new();
    let flush = |raw_seg: &mut Vec<String>,
                 cands: &mut Vec<(Vec<String>, Vec<String>, bool)>,
                 closed_by_rule: bool| {
        let cand = dialog_chrome_filtered(raw_seg);
        if !cand.is_empty() {
            cands.push((cand, raw_seg.clone(), closed_by_rule));
        }
        raw_seg.clear();
    };
    for (i, l) in lines.iter().enumerate() {
        let next_non_blank = lines[i + 1..]
            .iter()
            .find(|s| !s.trim().is_empty())
            .map(String::as_str);
        if is_dialog_boundary(l, next_non_blank) {
            flush(&mut raw_seg, &mut cands, is_rule(l));
            continue;
        }
        raw_seg.push(l.clone());
    }
    flush(&mut raw_seg, &mut cands, false);
    let Some((mut cleaned, mut raw, mut gated)) = cands.pop() else {
        return (Vec::new(), Vec::new());
    };
    loop {
        if gated {
            let openers = cleaned.iter().take_while(|l| is_input_opener(l)).count();
            cleaned.drain(..openers);
        }
        if !cleaned.is_empty() && !is_footer_segment(&cleaned) {
            break;
        }
        match cands.pop() {
            Some((prev_clean, prev_raw, prev_rule)) => {
                cleaned = prev_clean;
                raw = prev_raw;
                gated = prev_rule;
            }
            None => return (Vec::new(), Vec::new()),
        }
    }
    (cleaned, raw)
}
