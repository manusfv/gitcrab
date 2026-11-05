/// Insert at a boundary in the original order, keeping the same item active.
pub(super) fn move_to_slot<T>(
    tabs: &mut Vec<T>,
    active: &mut usize,
    from: usize,
    slot: usize,
) -> bool {
    if from >= tabs.len() || slot > tabs.len() {
        return false;
    }
    let to = slot - usize::from(from < slot);
    if from == to {
        return false;
    }
    let tab = tabs.remove(from);
    tabs.insert(to, tab);
    if *active == from {
        *active = to;
    } else if from < *active && *active <= to {
        *active -= 1;
    } else if to <= *active && *active < from {
        *active += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drop_boundary_preserves_order_and_active_identity() {
        for len in 1..=6 {
            for from in 0..len {
                for slot in 0..=len {
                    for selected in 0..len {
                        let mut tabs: Vec<_> = (0..len).collect();
                        let mut active = selected;
                        move_to_slot(&mut tabs, &mut active, from, slot);
                        assert_eq!(tabs[active], selected);
                        let remaining: Vec<_> =
                            tabs.iter().copied().filter(|t| *t != from).collect();
                        let expected: Vec<_> = (0..len).filter(|t| *t != from).collect();
                        assert_eq!(remaining, expected);
                        let to = slot - usize::from(from < slot);
                        assert_eq!(tabs[to], from);
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_and_unchanged_drops_leave_tabs_untouched() {
        for (from, slot) in [(3, 0), (0, 4), (1, 1), (1, 2)] {
            let mut tabs = vec![0, 1, 2];
            let mut active = 1;
            assert!(!move_to_slot(&mut tabs, &mut active, from, slot));
            assert_eq!(tabs, [0, 1, 2]);
            assert_eq!(active, 1);
        }
        assert!(!move_to_slot::<u8>(&mut vec![], &mut 0, 0, 0));
    }
}
