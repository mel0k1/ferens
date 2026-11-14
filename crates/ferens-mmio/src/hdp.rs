use ferens_core::BarOps;

use crate::vm;

pub const HDP_MEM_COHERENCY_FLUSH_CNTL: u32 = 0x1520;

pub fn flush<B: BarOps>(mmio: &ferens_core::Mmio<B>) {
    mmio.write_reg_direct(HDP_MEM_COHERENCY_FLUSH_CNTL, 1);
    let _ = mmio.read_reg_direct(HDP_MEM_COHERENCY_FLUSH_CNTL);
}

pub fn invalidate_with_flush<B: BarOps>(mmio: &ferens_core::Mmio<B>, vmid: u32) {
    flush(mmio);
    vm::invalidate(mmio, vmid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferens_core::Mmio;
    use std::cell::RefCell;

    struct FakeBar {
        cells: RefCell<Vec<u32>>,
        writes: RefCell<Vec<(usize, u32)>>,
    }

    impl FakeBar {
        fn new(words: usize) -> Self {
            Self {
                cells: RefCell::new(vec![0; words]),
                writes: RefCell::new(Vec::new()),
            }
        }
    }

    impl BarOps for FakeBar {
        fn read_u32(&self, offset: usize) -> u32 {
            self.cells.borrow()[offset / 4]
        }

        fn write_u32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset, value));
            self.cells.borrow_mut()[offset / 4] = value;
        }

        fn size(&self) -> usize {
            0x40000
        }
    }

    #[test]
    fn flush_writes_one_and_reads_back() {
        let mmio = Mmio::new(FakeBar::new(0x10000));
        flush(&mmio);
        assert_eq!(
            mmio.bar().writes.borrow().as_slice(),
            &[(HDP_MEM_COHERENCY_FLUSH_CNTL as usize * 4, 1)]
        );
    }

    #[test]
    fn invalidate_goes_after_hdp_flush() {
        let mmio = Mmio::new(FakeBar::new(0x10000));
        invalidate_with_flush(&mmio, 0);
        let writes = mmio.bar().writes.borrow();
        let flush_off = writes
            .iter()
            .position(|entry| entry.0 == HDP_MEM_COHERENCY_FLUSH_CNTL as usize * 4)
            .unwrap();
        let inv_off = writes
            .iter()
            .position(|entry| entry.0 == crate::vm::VM_INVALIDATE_REQUEST as usize * 4)
            .unwrap();
        assert!(flush_off < inv_off);
    }
}
