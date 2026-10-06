//! Generation-tagged slot storage, shared by the hosts that hand out handles.
//!
//! A [`SlotId`] stays meaningful after the value it named is removed: it
//! resolves to `None` rather than to whichever value later reuses the slot.
//! Slots are reused, so churn does not grow the roster.

/// Names one value in a `Roster`. Safe to hold past its removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotId {
    index: u32,
    generation: u32,
}

/// One slot. `generation` lives on the slot rather than the value so it keeps
/// counting while the slot is empty.
struct Slot<T> {
    value: Option<T>,
    generation: u32,
}

/// Values addressable by [`SlotId`].
pub struct Roster<T> {
    /// Values by slot. Empty slots are reused, so this does not grow with churn.
    slots: Vec<Slot<T>>,
    /// Indices of empty slots, so `insert` is O(1).
    free: Vec<u32>,
}

// Derived `Default` would demand `T: Default`, which no value here has.
impl<T> Default for Roster<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), free: Vec::new() }
    }
}

impl<T> Roster<T> {
    /// Store `value` and return the id naming it.
    pub fn insert(&mut self, value: T) -> SlotId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            return SlotId { index, generation: slot.generation };
        }
        let index = self.slots.len() as u32;
        self.slots.push(Slot { value: Some(value), generation: 0 });
        SlotId { index, generation: 0 }
    }

    /// Drop the value `id` names. Returns whether it named a live one; a stale
    /// id is an inert no-op.
    pub fn remove(&mut self, id: SlotId) -> bool {
        let Some(slot) = self.slots.get_mut(id.index as usize) else { return false };
        if slot.generation != id.generation || slot.value.is_none() {
            return false;
        }
        slot.value = None;
        // Retiring the generation stops the next value in this slot from
        // answering to the id just released.
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(id.index);
        true
    }

    /// The value `id` names, or `None` once it has been removed.
    pub fn get_mut(&mut self, id: SlotId) -> Option<&mut T> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation { None } else { slot.value.as_mut() }
    }

    /// Every live value, in slot order. Holes left by removals are skipped.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.slots.iter_mut().filter_map(|s| s.value.as_mut())
    }

    /// How many live values the roster holds.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.value.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle outliving its value resolves to "gone", not to whatever took
    /// over the slot.
    #[test]
    fn a_removed_id_never_names_its_successor() {
        let mut roster: Roster<u32> = Roster::default();

        let first = roster.insert(1);
        assert_eq!(roster.get_mut(first).copied(), Some(1));
        assert_eq!(roster.len(), 1);

        assert!(roster.remove(first), "removing a live value reported failure");
        assert!(roster.get_mut(first).is_none(), "a removed id still resolved");
        assert_eq!(roster.len(), 0);
        // A double destroy must be inert: no panic, no second slot release.
        assert!(!roster.remove(first), "removing the same id twice reported success");

        // The slot comes back, but the id does not.
        let second = roster.insert(2);
        assert_eq!(second.index, first.index, "the empty slot was not reused");
        assert_ne!(second.generation, first.generation);
        assert!(roster.get_mut(first).is_none(), "the old id resolved to the value that reused its slot");
        assert_eq!(roster.get_mut(second).copied(), Some(2));
        assert_eq!(roster.len(), 1);
    }

    /// Live values are handed out in slot order; holes left by removed ones are
    /// skipped rather than yielded as blanks.
    #[test]
    fn only_live_values_are_iterated() {
        let mut roster: Roster<u32> = Roster::default();
        let ids: Vec<SlotId> = (0..4).map(|i| roster.insert(i)).collect();
        assert_eq!(roster.iter_mut().count(), 4);

        roster.remove(ids[1]);
        roster.remove(ids[2]);
        assert_eq!(roster.len(), 2);
        assert_eq!(roster.iter_mut().map(|v| *v).collect::<Vec<_>>(), vec![0, 3]);

        // Refilling the holes must not produce duplicates or lose anyone.
        roster.insert(4);
        roster.insert(5);
        roster.insert(6);
        assert_eq!(roster.len(), 5);
        assert_eq!(roster.iter_mut().count(), 5);
    }
}
