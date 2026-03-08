/// A handle is two integers - a slot index and a generation counter.
/// Cheap to copy, cheap to pass. Stale handles return None, never panic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    pub slot: u32,
    pub gen: u32,
}

struct Slot<T> {
    value: Option<T>,
    gen: u32,
    #[allow(dead_code)]
    region_id: u32, // Phase 5b - bulk arena invalidation, unused for now
}

pub struct HandleRegistry<T> {
    slots: Vec<Slot<T>>,
    free_list: Vec<u32>,
}

impl<T> HandleRegistry<T> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
        }
    }

    /// Store a value. Returns a handle to retrieve it later.
    pub fn insert(&mut self, value: T) -> Handle {
        if let Some(slot_idx) = self.free_list.pop() {
            let slot = &mut self.slots[slot_idx as usize];
            slot.value = Some(value);
            // gen already bumped on remove - use it as-is
            Handle {
                slot: slot_idx,
                gen: slot.gen,
            }
        } else {
            let slot_idx = self.slots.len() as u32;
            self.slots.push(Slot {
                value: Some(value),
                gen: 0,
                region_id: 0,
            });
            Handle {
                slot: slot_idx,
                gen: 0,
            }
        }
    }

    /// Returns a reference if the handle is still valid. None if stale.
    pub fn get(&self, handle: Handle) -> Option<&T> {
        let slot = self.slots.get(handle.slot as usize)?;
        if slot.gen != handle.gen {
            return None; // stale - generation mismatch
        }
        slot.value.as_ref()
    }

    /// Returns a mutable reference if the handle is still valid. None if stale.
    #[allow(dead_code)]
    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.slot as usize)?;
        if slot.gen != handle.gen {
            return None; // stale - generation mismatch
        }
        slot.value.as_mut()
    }

    /// Frees the slot and bumps the generation.
    /// Any existing handles to this slot are now stale.
    pub fn remove(&mut self, handle: Handle) -> Option<T> {
        let slot = self.slots.get_mut(handle.slot as usize)?;
        if slot.gen != handle.gen {
            return None; // already freed or stale
        }
        let value = slot.value.take();
        slot.gen += 1; // bump - old handles are now dead
        self.free_list.push(handle.slot);
        value
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.value.is_some()).count()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h = reg.insert(42);
        assert_eq!(reg.get(h), Some(&42));
    }

    #[test]
    fn get_mut() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h = reg.insert(10);
        *reg.get_mut(h).unwrap() = 99;
        assert_eq!(reg.get(h), Some(&99));
    }

    #[test]
    fn remove_returns_value() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h = reg.insert(7);
        assert_eq!(reg.remove(h), Some(7));
    }

    #[test]
    fn stale_handle_returns_none_after_remove() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h = reg.insert(1);
        reg.remove(h);
        assert_eq!(reg.get(h), None);
    }

    #[test]
    fn stale_handle_after_slot_reuse() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h1 = reg.insert(1);
        reg.remove(h1);
        let h2 = reg.insert(2); // reuses slot 0
                                // h1 and h2 point to same slot but different generations
        assert_ne!(h1.gen, h2.gen);
        assert_eq!(reg.get(h1), None); // h1 is stale
        assert_eq!(reg.get(h2), Some(&2)); // h2 is valid
    }

    #[test]
    fn remove_stale_handle_returns_none() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h = reg.insert(5);
        reg.remove(h);
        assert_eq!(reg.remove(h), None); // double remove - safe
    }

    #[test]
    fn region_id_defaults_to_zero() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h = reg.insert(99);
        assert_eq!(reg.slots[h.slot as usize].region_id, 0);
    }

    #[test]
    fn multiple_inserts_unique_handles() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        let h1 = reg.insert(1);
        let h2 = reg.insert(2);
        let h3 = reg.insert(3);
        assert_ne!(h1, h2);
        assert_ne!(h2, h3);
        assert_eq!(reg.get(h1), Some(&1));
        assert_eq!(reg.get(h2), Some(&2));
        assert_eq!(reg.get(h3), Some(&3));
    }

    #[test]
    fn len_tracks_correctly() {
        let mut reg: HandleRegistry<i32> = HandleRegistry::new();
        assert_eq!(reg.len(), 0);
        let h1 = reg.insert(1);
        let h2 = reg.insert(2);
        assert_eq!(reg.len(), 2);
        reg.remove(h1);
        assert_eq!(reg.len(), 1);
        reg.remove(h2);
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
    }
}
