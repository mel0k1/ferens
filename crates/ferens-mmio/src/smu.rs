use std::io;
use std::thread;
use std::time::{Duration, Instant};

use ferens_core::{BarOps, Mmio};

// Регистры SMC-блока — dword-индексы из smu_7_1_3_d.h.
pub const SMC_IND_INDEX: u32 = 0x80;
pub const SMC_IND_DATA: u32 = 0x81;
pub const SMC_MESSAGE_0: u32 = 0x94;
pub const SMC_RESP_0: u32 = 0x95;
pub const SMC_MSG_ARG_0: u32 = 0xa4;
pub const SMC_RESP_MASK: u32 = 0xffff;

// Косвенные адреса SMC SRAM — из smu_7_1_3_d.h.
pub const IX_SMC_SYSCON_CLOCK_CNTL_0: u32 = 0x8000_0004;
pub const IX_SMC_PC_C: u32 = 0x8000_0370;
// Условия smu7_is_smc_ram_running (smu7_smumgr.c): клок не заглох и PC в прошивке.
pub const SMC_CLOCK_CNTL_0_CK_DISABLE_BIT: u32 = 1;
pub const SMC_PC_C_MIN: u32 = 0x20100;

// Заголовок прошивки SMU: smu73.h, SMU7_FIRMWARE_HEADER_LOCATION + SMU73_Firmware_Header.
pub const SMU7_FIRMWARE_HEADER_LOCATION: u32 = 0x20000;
pub const FIRMWARE_HDR_DPM_TABLE_OFFSET: u32 = 52;
// SMU73_Discrete_DpmTable — смещения полей из компиляции smu73_discrete.h.
pub const DPM_GRAPHICS_LEVEL_COUNT_OFFSET: u32 = 208;
pub const DPM_GRAPHICS_LEVELS_OFFSET: u32 = 236;
pub const DPM_MEMORY_LEVEL_COUNT_OFFSET: u32 = 209;
pub const DPM_MEMORY_LEVELS_OFFSET: u32 = 612;
pub const GRAPHICS_LEVEL_SIZE: u32 = 44;
pub const GRAPHICS_LEVEL_SCLK_FREQUENCY_OFFSET: u32 = 4;
pub const MEMORY_LEVEL_SIZE: u32 = 24;
pub const MEMORY_LEVEL_MCLK_FREQUENCY_OFFSET: u32 = 8;
pub const SMU73_MAX_LEVELS_GRAPHICS: usize = 8;
pub const SMU73_MAX_LEVELS_MEMORY: usize = 4;
pub const SMU7_SMC_SIZE: u32 = 0x20000;

// Коды результатов и сообщения из smu7_ppsmc.h.
pub const PPSMC_RESULT_OK: u32 = 0x01;
pub const PPSMC_RESULT_UNKNOWN_CMD: u32 = 0xfe;
pub const PPSMC_RESULT_NO_FW: u32 = 0xffff;
pub const MSG_API_GET_SCLK_FREQUENCY: u32 = 0x200;
pub const MSG_API_GET_MCLK_FREQUENCY: u32 = 0x201;
pub const MSG_SCLKDPM_SET_ENABLED_MASK: u32 = 0x145;
pub const MSG_MCLKDPM_SET_ENABLED_MASK: u32 = 0x146;
pub const MSG_PCIE_DPM_UNFORCE_LEVEL: u32 = 0x148;

// Таймаут ожидания ответа — как adev->usec_timeout (100 мс) в ядре.
pub const RESP_TIMEOUT: Duration = Duration::from_millis(100);

pub const DPM_TABLE_EMPTY: &str = "SMU dpm table is empty";

pub fn mhz(freq_10khz: u32) -> u32 {
    freq_10khz / 100
}

#[derive(Debug, Clone)]
pub struct DpmTables {
    pub sclk_levels: Vec<u32>,
    pub mclk_levels: Vec<u32>,
}

impl DpmTables {
    pub fn sclk_mhz(&self) -> Vec<u32> {
        self.sclk_levels.iter().map(|&freq| mhz(freq)).collect()
    }

    pub fn mclk_mhz(&self) -> Vec<u32> {
        self.mclk_levels.iter().map(|&freq| mhz(freq)).collect()
    }

    pub fn sclk_mask(&self) -> u32 {
        if self.sclk_levels.is_empty() {
            0
        } else {
            (1 << self.sclk_levels.len()) - 1
        }
    }

    pub fn mclk_mask(&self) -> u32 {
        if self.mclk_levels.is_empty() {
            0
        } else {
            (1 << self.mclk_levels.len()) - 1
        }
    }

    pub fn report(&self) -> String {
        let sclk = self
            .sclk_mhz()
            .iter()
            .map(|mhz| mhz.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let mclk = self
            .mclk_mhz()
            .iter()
            .map(|mhz| mhz.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "sclk dpm {} уровней: {} MHz\nmclk dpm {} уровней: {} MHz",
            self.sclk_levels.len(),
            sclk,
            self.mclk_levels.len(),
            mclk
        )
    }
}

pub struct SmuController<'a, B: BarOps> {
    regs: &'a Mmio<B>,
    poll_timeout: Duration,
}

impl<'a, B: BarOps> SmuController<'a, B> {
    pub fn new(regs: &'a Mmio<B>) -> io::Result<Self> {
        if !Self::ram_running(regs) {
            return Err(io::Error::other(
                "SMC ram is not running (clock gated or smu firmware absent)",
            ));
        }
        Ok(Self {
            regs,
            poll_timeout: RESP_TIMEOUT,
        })
    }

    fn ram_running(regs: &Mmio<B>) -> bool {
        regs.write_reg_direct(SMC_IND_INDEX, IX_SMC_SYSCON_CLOCK_CNTL_0);
        let clock_cntl = regs.read_reg_direct(SMC_IND_DATA);
        regs.write_reg_direct(SMC_IND_INDEX, IX_SMC_PC_C);
        let pc = regs.read_reg_direct(SMC_IND_DATA);
        (clock_cntl & SMC_CLOCK_CNTL_0_CK_DISABLE_BIT) == 0 && pc >= SMC_PC_C_MIN
    }

    fn read_smc(&self, address: u32) -> u32 {
        self.regs.write_reg_direct(SMC_IND_INDEX, address);
        self.regs.read_reg_direct(SMC_IND_DATA)
    }

    fn wait_resp(&self) -> io::Result<u32> {
        let started = Instant::now();
        loop {
            let resp = self.regs.read_reg_direct(SMC_RESP_0) & SMC_RESP_MASK;
            if resp != 0 {
                return Ok(resp);
            }
            if started.elapsed() > self.poll_timeout {
                return Err(io::Error::other("SMU response did not arrive in time"));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    // smu7_send_msg_to_smc (smu7_smumgr.c): дождаться RESP != 0, сбросить,
    // записать SMC_MESSAGE_0, дождаться нового RESP и разобрать код.
    pub fn send_msg(&self, msg: u32) -> io::Result<()> {
        self.wait_resp()?;
        self.regs.write_reg_direct(SMC_RESP_0, 0);
        self.regs.write_reg_direct(SMC_MESSAGE_0, msg);
        let resp = self.wait_resp()?;
        match resp {
            PPSMC_RESULT_OK => Ok(()),
            PPSMC_RESULT_UNKNOWN_CMD => Err(io::Error::other(format!(
                "SMU rejected message {msg:#x}: unknown command"
            ))),
            PPSMC_RESULT_NO_FW => Err(io::Error::other(format!(
                "SMU rejected message {msg:#x}: firmware not loaded"
            ))),
            other => Err(io::Error::other(format!(
                "SMU message {msg:#x} failed: response {other:#x}"
            ))),
        }
    }

    pub fn send_msg_with_parameter(&self, msg: u32, parameter: u32) -> io::Result<()> {
        self.regs.write_reg_direct(SMC_MSG_ARG_0, parameter);
        self.send_msg(msg)
    }

    // smu7_get_argument: ответ без кода результата лежит в SMC_MSG_ARG_0.
    pub fn argument(&self) -> u32 {
        self.regs.read_reg_direct(SMC_MSG_ARG_0)
    }

    // PPSMC_MSG_API_GetSclkFrequency: частота в единицах 10 кГц.
    pub fn current_sclk(&self) -> io::Result<u32> {
        self.send_msg(MSG_API_GET_SCLK_FREQUENCY)?;
        Ok(self.argument())
    }

    pub fn current_mclk(&self) -> io::Result<u32> {
        self.send_msg(MSG_API_GET_MCLK_FREQUENCY)?;
        Ok(self.argument())
    }

    // DPM-таблица живёт в SMC SRAM: адрес берётся из заголовка прошивки,
    // количества уровней - байты GraphicsDpmLevelCount/MemoryDpmLevelCount,
    // частоты - SclkFrequency/MclkFrequency внутри уровней.
    pub fn dpm_tables(&self) -> io::Result<DpmTables> {
        let header = SMU7_FIRMWARE_HEADER_LOCATION;
        let dpm = self.read_smc(header + FIRMWARE_HDR_DPM_TABLE_OFFSET);
        if dpm == 0 || dpm >= SMU7_SMC_SIZE {
            return Err(io::Error::other(format!(
                "SMU dpm table address {dpm:#x} from firmware header is bogus"
            )));
        }
        let gcount = (self.read_smc(dpm + DPM_GRAPHICS_LEVEL_COUNT_OFFSET) & 0xff) as usize;
        let gcount = gcount.min(SMU73_MAX_LEVELS_GRAPHICS);
        let mut sclk_levels = Vec::with_capacity(gcount);
        for index in 0..gcount as u32 {
            let address = dpm
                + DPM_GRAPHICS_LEVELS_OFFSET
                + index * GRAPHICS_LEVEL_SIZE
                + GRAPHICS_LEVEL_SCLK_FREQUENCY_OFFSET;
            sclk_levels.push(self.read_smc(address));
        }
        let mcount = (self.read_smc(dpm + DPM_MEMORY_LEVEL_COUNT_OFFSET) & 0xff) as usize;
        let mcount = mcount.min(SMU73_MAX_LEVELS_MEMORY);
        let mut mclk_levels = Vec::with_capacity(mcount);
        for index in 0..mcount as u32 {
            let address = dpm
                + DPM_MEMORY_LEVELS_OFFSET
                + index * MEMORY_LEVEL_SIZE
                + MEMORY_LEVEL_MCLK_FREQUENCY_OFFSET;
            mclk_levels.push(self.read_smc(address));
        }
        if sclk_levels.is_empty() || mclk_levels.is_empty() {
            return Err(io::Error::other(DPM_TABLE_EMPTY));
        }
        Ok(DpmTables {
            sclk_levels,
            mclk_levels,
        })
    }

    // smu7_upload_dpm_level_enable_mask: маска битов по числу уровней.
    pub fn set_sclk_enabled_mask(&self, mask: u32) -> io::Result<()> {
        self.send_msg_with_parameter(MSG_SCLKDPM_SET_ENABLED_MASK, mask)
    }

    pub fn set_mclk_enabled_mask(&self, mask: u32) -> io::Result<()> {
        self.send_msg_with_parameter(MSG_MCLKDPM_SET_ENABLED_MASK, mask)
    }

    // smu7_force_dpm_highest/lowest: маска из одного бита уровня.
    pub fn force_sclk_highest(&self, tables: &DpmTables) -> io::Result<()> {
        let level = tables.sclk_levels.len() - 1;
        self.set_sclk_enabled_mask(1 << level)
    }

    pub fn force_sclk_lowest(&self) -> io::Result<()> {
        self.set_sclk_enabled_mask(1)
    }

    pub fn force_mclk_highest(&self, tables: &DpmTables) -> io::Result<()> {
        let level = tables.mclk_levels.len() - 1;
        self.set_mclk_enabled_mask(1 << level)
    }

    pub fn force_mclk_lowest(&self) -> io::Result<()> {
        self.set_mclk_enabled_mask(1)
    }

    // smu7_unforce_dpm_levels: разфорс PCIe, затем полные маски sclk/mclk.
    pub fn unforce(&self, tables: &DpmTables) -> io::Result<()> {
        self.send_msg(MSG_PCIE_DPM_UNFORCE_LEVEL)?;
        let sclk_mask = tables.sclk_mask();
        if sclk_mask != 0 {
            self.set_sclk_enabled_mask(sclk_mask)?;
        }
        let mclk_mask = tables.mclk_mask();
        if mclk_mask != 0 {
            self.set_mclk_enabled_mask(mclk_mask)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const DPM_BASE: u32 = 0x0001_2340;

    struct SmuBar {
        ind_addr: Cell<u32>,
        resp: Cell<u32>,
        arg: Cell<u32>,
        next_resp: Cell<u32>,
        clock_cntl: Cell<u32>,
        pc_c: Cell<u32>,
        messages: RefCell<Vec<u32>>,
        params: RefCell<Vec<u32>>,
    }

    impl SmuBar {
        fn new() -> Self {
            Self {
                ind_addr: Cell::new(0),
                resp: Cell::new(1),
                arg: Cell::new(0),
                next_resp: Cell::new(PPSMC_RESULT_OK),
                clock_cntl: Cell::new(0),
                pc_c: Cell::new(SMC_PC_C_MIN),
                messages: RefCell::new(Vec::new()),
                params: RefCell::new(Vec::new()),
            }
        }

        fn read_sram(&self, address: u32) -> u32 {
            if address == IX_SMC_SYSCON_CLOCK_CNTL_0 {
                self.clock_cntl.get()
            } else if address == IX_SMC_PC_C {
                self.pc_c.get()
            } else if address == SMU7_FIRMWARE_HEADER_LOCATION + FIRMWARE_HDR_DPM_TABLE_OFFSET {
                DPM_BASE
            } else if address == DPM_BASE + DPM_GRAPHICS_LEVEL_COUNT_OFFSET {
                3
            } else if address == DPM_BASE + DPM_MEMORY_LEVEL_COUNT_OFFSET {
                2
            } else if address
                == DPM_BASE + DPM_GRAPHICS_LEVELS_OFFSET + GRAPHICS_LEVEL_SCLK_FREQUENCY_OFFSET
            {
                30000
            } else if address
                == DPM_BASE
                    + DPM_GRAPHICS_LEVELS_OFFSET
                    + GRAPHICS_LEVEL_SIZE
                    + GRAPHICS_LEVEL_SCLK_FREQUENCY_OFFSET
            {
                50000
            } else if address
                == DPM_BASE
                    + DPM_GRAPHICS_LEVELS_OFFSET
                    + 2 * GRAPHICS_LEVEL_SIZE
                    + GRAPHICS_LEVEL_SCLK_FREQUENCY_OFFSET
            {
                70000
            } else if address
                == DPM_BASE + DPM_MEMORY_LEVELS_OFFSET + MEMORY_LEVEL_MCLK_FREQUENCY_OFFSET
            {
                12500
            } else if address
                == DPM_BASE
                    + DPM_MEMORY_LEVELS_OFFSET
                    + MEMORY_LEVEL_SIZE
                    + MEMORY_LEVEL_MCLK_FREQUENCY_OFFSET
            {
                15000
            } else {
                0
            }
        }
    }

    // BarOps оперирует байтовыми смещениями — переводим dword-индексы SMC-блока.
    const SMC_IND_INDEX_BYTE: u32 = SMC_IND_INDEX * 4;
    const SMC_IND_DATA_BYTE: u32 = SMC_IND_DATA * 4;
    const SMC_MESSAGE_BYTE: u32 = SMC_MESSAGE_0 * 4;
    const SMC_RESP_BYTE: u32 = SMC_RESP_0 * 4;
    const SMC_MSG_ARG_BYTE: u32 = SMC_MSG_ARG_0 * 4;

    impl BarOps for SmuBar {
        fn read_u32(&self, offset: usize) -> u32 {
            match offset as u32 {
                SMC_IND_INDEX_BYTE => self.ind_addr.get(),
                SMC_IND_DATA_BYTE => self.read_sram(self.ind_addr.get()),
                SMC_RESP_BYTE => self.resp.get(),
                SMC_MSG_ARG_BYTE => self.arg.get(),
                _ => 0,
            }
        }

        fn write_u32(&self, offset: usize, value: u32) {
            match offset as u32 {
                SMC_IND_INDEX_BYTE => self.ind_addr.set(value),
                SMC_RESP_BYTE => self.resp.set(value),
                SMC_MESSAGE_BYTE => {
                    self.messages.borrow_mut().push(value);
                    self.resp.set(self.next_resp.get());
                }
                SMC_MSG_ARG_BYTE => {
                    self.arg.set(value);
                    self.params.borrow_mut().push(value);
                }
                _ => {}
            }
        }

        fn size(&self) -> usize {
            0x40000
        }
    }

    #[test]
    fn controller_rejects_gated_smc_ram() {
        let bar = SmuBar::new();
        bar.clock_cntl.set(SMC_CLOCK_CNTL_0_CK_DISABLE_BIT);
        let mmio = Mmio::new(bar);
        match SmuController::new(&mmio) {
            Ok(_) => panic!("gated SMC ram should be rejected"),
            Err(error) => assert!(error.to_string().contains("SMC ram is not running")),
        }
    }

    #[test]
    fn send_msg_follows_kernel_handshake_and_parses_response() {
        let bar = SmuBar::new();
        let mmio = Mmio::new(bar);
        SmuController::new(&mmio).unwrap().send_msg(0x77).unwrap();
        assert_eq!(mmio.bar().messages.borrow().as_slice(), &[0x77]);
        assert_eq!(mmio.bar().resp.get(), PPSMC_RESULT_OK);
    }

    #[test]
    fn unknown_command_and_no_fw_are_reported() {
        let bar = SmuBar::new();
        bar.next_resp.set(PPSMC_RESULT_UNKNOWN_CMD);
        let mmio = Mmio::new(bar);
        let error = SmuController::new(&mmio)
            .unwrap()
            .send_msg(0x77)
            .unwrap_err();
        assert!(error.to_string().contains("unknown command"));
        let bar = SmuBar::new();
        bar.next_resp.set(PPSMC_RESULT_NO_FW);
        let mmio = Mmio::new(bar);
        let error = SmuController::new(&mmio)
            .unwrap()
            .send_msg(0x77)
            .unwrap_err();
        assert!(error.to_string().contains("firmware not loaded"));
    }
    #[test]
    fn current_clocks_read_argument_after_api_message() {
        let bar = SmuBar::new();
        bar.arg.set(61250);
        let mmio = Mmio::new(bar);
        let smu = SmuController::new(&mmio).unwrap();
        assert_eq!(smu.current_sclk().unwrap(), 61250);
        assert_eq!(smu.current_mclk().unwrap(), 61250);
        assert_eq!(mhz(61250), 612);
        assert_eq!(mmio.bar().messages.borrow().len(), 2);
    }

    #[test]
    fn dpm_tables_read_counts_and_level_frequencies_from_sram() {
        let bar = SmuBar::new();
        let mmio = Mmio::new(bar);
        let smu = SmuController::new(&mmio).unwrap();
        let tables = smu.dpm_tables().unwrap();
        assert_eq!(tables.sclk_levels, vec![30000, 50000, 70000]);
        assert_eq!(tables.mclk_levels, vec![12500, 15000]);
        assert_eq!(tables.sclk_mhz(), vec![300, 500, 700]);
        assert_eq!(tables.mclk_mhz(), vec![125, 150]);
        assert_eq!(tables.sclk_mask(), 0b111);
        assert_eq!(tables.mclk_mask(), 0b11);
        assert!(tables.report().contains("sclk dpm 3 уровней"));
    }

    #[test]
    fn force_masks_and_unforce_send_kernel_parameters() {
        let bar = SmuBar::new();
        let mmio = Mmio::new(bar);
        let smu = SmuController::new(&mmio).unwrap();
        let tables = smu.dpm_tables().unwrap();
        smu.force_sclk_highest(&tables).unwrap();
        smu.force_sclk_lowest().unwrap();
        smu.force_mclk_highest(&tables).unwrap();
        smu.force_mclk_lowest().unwrap();
        smu.unforce(&tables).unwrap();
        assert_eq!(
            mmio.bar().params.borrow().as_slice(),
            &[
                0b100, // sclk highest = 1 << 2
                0b001, // sclk lowest
                0b10,  // mclk highest = 1 << 1
                0b01,  // mclk lowest
                0b111, // unforce: полная маска sclk
                0b11,  // полная маска mclk
            ]
        );
        // unforce дополнительно посылает PCIeDPM_UnForceLevel
        let messages = mmio.bar().messages.borrow();
        assert!(messages.contains(&MSG_PCIE_DPM_UNFORCE_LEVEL));
    }
}
