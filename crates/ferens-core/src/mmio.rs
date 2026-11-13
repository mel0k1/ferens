use crate::bar::BarOps;

pub const MM_INDEX_BYTE_OFFSET: usize = 0x0;
pub const MM_DATA_BYTE_OFFSET: usize = 0x4;

pub struct Mmio<B: BarOps> {
    bar: B,
}

impl<B: BarOps> Mmio<B> {
    pub fn new(bar: B) -> Self {
        Self { bar }
    }

    pub fn bar(&self) -> &B {
        &self.bar
    }

    pub fn into_bar(self) -> B {
        self.bar
    }

    pub fn read_reg(&self, offset: u32) -> u32 {
        self.bar.write_u32(MM_INDEX_BYTE_OFFSET, offset);
        self.bar.read_u32(MM_DATA_BYTE_OFFSET)
    }

    pub fn write_reg(&self, offset: u32, value: u32) {
        self.bar.write_u32(MM_INDEX_BYTE_OFFSET, offset);
        self.bar.write_u32(MM_DATA_BYTE_OFFSET, value);
    }

    pub fn read_reg_direct(&self, offset: u32) -> u32 {
        self.bar.read_u32(offset as usize * 4)
    }

    pub fn write_reg_direct(&self, offset: u32, value: u32) {
        self.bar.write_u32(offset as usize * 4, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            16
        }
    }

    #[test]
    fn write_reg_sends_index_then_data() {
        let mmio = Mmio::new(FakeBar::new(4));
        mmio.write_reg(0xC200, 0x5);
        assert_eq!(
            mmio.bar().writes.borrow().as_slice(),
            &[(0x0, 0xC200), (0x4, 0x5)]
        );
    }

    #[test]
    fn read_reg_addresses_then_fetches_data() {
        let mmio = Mmio::new(FakeBar::new(4));
        mmio.bar().cells.borrow_mut()[1] = 0xABCD_1234;
        assert_eq!(mmio.read_reg(0x150A), 0xABCD_1234);
        assert_eq!(mmio.bar().writes.borrow().as_slice(), &[(0x0, 0x150A)]);
    }

    #[test]
    fn direct_access_multiplies_offset_by_word_size() {
        let mmio = Mmio::new(FakeBar::new(4));
        mmio.bar().cells.borrow_mut()[2] = 0x55;
        assert_eq!(mmio.read_reg_direct(2), 0x55);
        assert_eq!(mmio.bar().writes.borrow().as_slice(), &[]);
        mmio.write_reg_direct(3, 0x5C);
        assert_eq!(mmio.bar().cells.borrow()[3], 0x5C);
        assert_eq!(mmio.bar().writes.borrow().as_slice(), &[(0xC, 0x5C)]);
    }
}
