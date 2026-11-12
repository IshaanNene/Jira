//! Board order as fractional rank strings: moving an issue rewrites one row.
//!
//! A rank is a base-36 fraction ("i" = 0.i) compared bytewise; ranks never end in '0',
//! so there is always room between two different ranks. Appending and prepending step
//! by a fixed amount inside a six-digit block, which keeps ranks short for the common
//! "new issue goes to the bottom" case; inserting between two ranks takes their midpoint.

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const BLOCK: usize = 6;
const BLOCK_SPACE: u64 = 36u64.pow(BLOCK as u32);
const STEP: u64 = 36 * 36;

fn digit(c: u8) -> u64 {
    match c {
        b'0'..=b'9' => u64::from(c - b'0'),
        b'a'..=b'z' => u64::from(c - b'a') + 10,
        // Ranks are only ever produced here; anything else sorts as the lowest digit.
        _ => 0,
    }
}

/// The first six digits as a number (shorter ranks are padded with zeros).
fn block_value(rank: &str) -> u64 {
    let bytes = rank.as_bytes();
    (0..BLOCK).fold(0, |acc, i| acc * 36 + bytes.get(i).map_or(0, |c| digit(*c)))
}

fn encode_block(mut value: u64) -> String {
    let mut out = [b'0'; BLOCK];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS[(value % 36) as usize];
        value /= 36;
    }
    let end = out.iter().rposition(|c| *c != b'0').map_or(0, |i| i + 1);
    String::from_utf8_lossy(&out[..end]).into_owned()
}

/// A rank strictly between `a` and `b`, where `a < b`, `a` may be empty (lowest) and
/// `b = None` means unbounded.
fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        let common = b.iter().enumerate().take_while(|(i, c)| a.get(*i).copied().unwrap_or(b'0') == **c).count();
        if common > 0 {
            let mut out = b[..common].to_vec();
            out.extend(midpoint(a.get(common..).unwrap_or(&[]), Some(&b[common..])));
            return out;
        }
    }
    let low = a.first().map_or(0, |c| digit(*c));
    let high = b.and_then(<[u8]>::first).map_or(36, |c| digit(*c));
    if high - low > 1 {
        return vec![DIGITS[((low + high) / 2) as usize]];
    }
    match b {
        Some(b) if b.len() > 1 => vec![b[0]],
        _ => {
            let mut out = vec![DIGITS[low as usize]];
            out.extend(midpoint(a.get(1..).unwrap_or(&[]), None));
            out
        }
    }
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

fn after(a: &str) -> String {
    let next = block_value(a) + STEP;
    if next < BLOCK_SPACE {
        encode_block(next)
    } else {
        text(midpoint(a.as_bytes(), None))
    }
}

fn before(b: &str) -> String {
    let value = block_value(b);
    if value > STEP {
        encode_block(value - STEP)
    } else {
        text(midpoint(&[], Some(b.as_bytes())))
    }
}

/// A rank between two neighbours; `None` means "no neighbour on that side".
pub fn between(lo: Option<&str>, hi: Option<&str>) -> String {
    match (lo, hi) {
        (None, None) => encode_block(BLOCK_SPACE / 2),
        (Some(lo), None) => after(lo),
        (None, Some(hi)) => before(hi),
        // Callers serialise rank changes per project, so equal or inverted neighbours
        // should not occur; if they do, placing after `lo` keeps the order usable.
        (Some(lo), Some(hi)) if lo >= hi => after(lo),
        (Some(lo), Some(hi)) => text(midpoint(lo.as_bytes(), Some(hi.as_bytes()))),
    }
}

/// `n` increasing ranks between two neighbours.
pub fn spread(lo: Option<&str>, hi: Option<&str>, n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    match (lo, hi) {
        (_, None) => {
            let mut last = lo.map(str::to_owned);
            for _ in 0..n {
                let next = between(last.as_deref(), None);
                out.push(next.clone());
                last = Some(next);
            }
        }
        (None, Some(hi)) => {
            let mut first = hi.to_owned();
            for _ in 0..n {
                first = between(None, Some(&first));
                out.push(first.clone());
            }
            out.reverse();
        }
        (Some(lo), Some(hi)) => bisect(lo, hi, n, &mut out),
    }
    out
}

fn bisect(lo: &str, hi: &str, n: usize, out: &mut Vec<String>) {
    if n == 0 {
        return;
    }
    let mid = between(Some(lo), Some(hi));
    let left = (n - 1) / 2;
    bisect(lo, &mid, left, out);
    out.push(mid.clone());
    bisect(&mid, hi, n - 1 - left, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_strictly_increasing(ranks: &[String]) {
        for pair in ranks.windows(2) {
            assert!(pair[0] < pair[1], "{:?} should sort before {:?}", pair[0], pair[1]);
        }
        assert!(ranks.iter().all(|r| !r.is_empty() && !r.ends_with('0')), "ranks never end in 0: {ranks:?}");
    }

    #[test]
    fn appending_thousands_keeps_ranks_short_and_ordered() {
        let ranks = spread(None, None, 5000);
        assert_strictly_increasing(&ranks);
        assert!(ranks.iter().all(|r| r.len() <= BLOCK));
    }

    #[test]
    fn prepending_stays_ordered_past_the_block_floor() {
        let mut ranks = vec![between(None, None)];
        for _ in 0..20_000 {
            let first = between(None, Some(&ranks[0]));
            ranks.insert(0, first);
        }
        assert_strictly_increasing(&ranks);
    }

    #[test]
    fn appending_stays_ordered_past_the_block_ceiling() {
        let mut last = "zzzzz".to_owned();
        let mut ranks = vec![last.clone()];
        for _ in 0..200 {
            last = between(Some(&last), None);
            ranks.push(last.clone());
        }
        assert_strictly_increasing(&ranks);
    }

    #[test]
    fn there_is_always_room_between_two_neighbours() {
        // Keep inserting directly after the first item: the tightest case.
        let (lo, mut hi) = ("i".to_owned(), "i1".to_owned());
        for _ in 0..300 {
            let mid = between(Some(&lo), Some(&hi));
            assert!(lo < mid && mid < hi, "{lo} < {mid} < {hi}");
            assert!(!mid.ends_with('0'));
            hi = mid;
        }
    }

    #[test]
    fn spread_fills_a_gap_in_order() {
        for (lo, hi) in [("a", "b"), ("i", "i001"), ("0001", "z"), ("az", "b")] {
            let ranks = spread(Some(lo), Some(hi), 64);
            assert_eq!(ranks.len(), 64);
            let mut all = vec![lo.to_owned()];
            all.extend(ranks);
            all.push(hi.to_owned());
            assert_strictly_increasing(&all);
        }
        let front = spread(None, Some("i"), 10);
        assert_strictly_increasing(&front);
        assert!(front[9].as_str() < "i");
    }

    #[test]
    fn mixed_moves_never_collide() {
        // Deterministic pseudo-random shuffle of 200 items, 3000 moves.
        let mut order = spread(None, None, 200);
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound as u64) as usize
        };
        for _ in 0..3000 {
            let from = next(order.len());
            order.remove(from);
            let to = next(order.len() + 1);
            let lo = to.checked_sub(1).map(|i| order[i].clone());
            let hi = order.get(to).cloned();
            order.insert(to, between(lo.as_deref(), hi.as_deref()));
            assert!(lo.as_deref().is_none_or(|l| l < order[to].as_str()));
            assert!(hi.as_deref().is_none_or(|h| order[to].as_str() < h));
        }
        assert_strictly_increasing(&order);
    }
}
