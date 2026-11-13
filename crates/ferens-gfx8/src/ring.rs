use crate::pm4::NOP_WORD;

pub struct Ring {
    words: Vec<u32>,
    write_dw: usize,
}

impl Ring {
    pub fn new(size_dw: usize) -> Option<Self> {
        if size_dw == 0 || !size_dw.is_power_of_two() {
            return None;
        }
        Some(Self {
            words: vec![0; size_dw],
            write_dw: 0,
        })
    }

    pub fn capacity_dw(&self) -> usize {
        self.words.len()
    }

    pub fn write_dw(&self) -> usize {
        self.write_dw
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.words
    }

    pub fn emit(&mut self, packet: &[u32]) -> bool {
        if packet.is_empty() || packet.len() > self.words.len() {
            return false;
        }
        let tail = self.words.len() - self.write_dw;
        if tail < packet.len() {
            for word in &mut self.words[self.write_dw..] {
                *word = NOP_WORD;
            }
            self.words[..packet.len()].copy_from_slice(packet);
            self.write_dw = packet.len();
        } else {
            let end = self.write_dw + packet.len();
            self.words[self.write_dw..end].copy_from_slice(packet);
            self.write_dw = end % self.words.len();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_and_unaligned_sizes() {
        assert!(Ring::new(0).is_none());
        assert!(Ring::new(7).is_none());
        assert!(Ring::new(8).is_some());
    }

    #[test]
    fn emit_fills_wrap_with_single_word_nops() {
        let mut ring = Ring::new(8).unwrap();
        assert!(ring.emit(&[0xC002_1000, 0, 0, 0, 0, 0]));
        assert_eq!(ring.write_dw(), 6);
        assert!(ring.emit(&[0xC003_1500, 8, 1, 1, 1]));
        assert_eq!(ring.write_dw(), 5);
        assert_eq!(ring.as_slice()[6], NOP_WORD);
        assert_eq!(ring.as_slice()[7], NOP_WORD);
        let head: Vec<u32> = ring.as_slice()[..5].to_vec();
        assert_eq!(head, [0xC003_1500, 8, 1, 1, 1]);
    }

    #[test]
    fn exact_fit_wraps_write_ptr_to_zero() {
        let mut ring = Ring::new(4).unwrap();
        assert!(ring.emit(&[0xC002_7600, 0x20C, 0xAABB, 0xCCDD]));
        assert_eq!(ring.write_dw(), 0);
    }

    #[test]
    fn emit_rejects_oversized_and_empty_packets() {
        let mut ring = Ring::new(8).unwrap();
        assert!(!ring.emit(&[0; 9]));
        assert!(!ring.emit(&[]));
    }
}
