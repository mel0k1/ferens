# Roadmap

Аудит «чего не хватает, чтобы зваться драйвером», разложенный по тирам. Тир закрывается только когда всё в нём сделано; порядок честный — снизу вверх.

## tier 0 — фундамент безопасности (закрыт)

Без этого нечего и трогать GPU: нельзя ни увидеть причину отказа, ни не уронить машину.

- [x] golden regs — таблицы golden-регистров gfx8/gmc8, портированные дословно из gfx_v8_0.c и gmc_v8_0.c (17+8 записей для polaris10, 17+6 для polaris11/12, 5+4 gmc), применяются первыми в init, до сборки CSB, RLC и MEC; для polaris10 ещё и SMC-твик ixCG_ACLK_CNTL.ACLK_DIVIDER = 0x18 через SMC_IND_INDEX/DATA
- [x] HDP flush — сброс host data path кэша GPU (HDP_MEM_COHERENCY_FLUSH_CNTL = 1 с readback) после каждой записи CPU в память, которую читает GPU: PTE, кольцо и мета перед doorbell, как vi_flush_hdp в vi.c
- [x] fault status — чтение и расшифровка VM_CONTEXT0/1_PROTECTION_FAULT_STATUS/ADDR/MCCLIENT (PROTECTIONS [7:0], MEMORY_CLIENT_ID [20:12], MEMORY_CLIENT_RW [24], VMID [28:25]) плюс GRBM_STATUS и SRBM_STATUS/SRBM_STATUS2: команда `status` и автодамп при таймауте fence, сброс через VM_CONTEXTn_CNTL2, как gmc_v8_0_process_interrupt
- [x] CRC — readback-верификация всего, что пишется в GPU через MMIO-окна: RLC ucode (RLC_GPM_UCODE_ADDR/DATA), MEC1/MEC2 ucode (CP_MEC_MEx_UCODE_ADDR/DATA), CSB в VRAM; несовпадение CRC32 — ошибка init, а не тихий мусор в SRAM
- [x] IOMMU guard — отказ на старте, если устройство сидит в /sys/kernel/iommu_groups: DMA в системную память уйдёт мимо наших PTE; обходной лаз — FERENS_IOMMU_ALLOW=1

## tier 1 — ucode и очередь (готово)

- [x] RLC ucode + SRM-список из rlc.bin, CSB из clearstate_vi.h, PG-инициализация на polaris11/12
- [x] MEC1/MEC2 ucode, CP_HQD через MMIO, doorbell, PM4 dispatch, fence polling
- [x] GTT: identity-окно VRAM + системные PTE из /proc/self/pagemap, alloc_gtt
- [x] GFX ucode (PFP/ME/CE) и gfx ring — три окна загрузки с CRC-верификацией, кольцо 4 КБ в VRAM с writeback rptr, кик через MMIO WPTR, WRITE_DATA-фенс как ring_test_ring, `ferens gfx-ring`; до clear-state-потока и отрисовки не доведено
- [x] SDMA ucode и кольцо — микрокод обоих инстансов с CRC-верификацией, GFX-кольца 2x4 КБ в VRAM, writeback rptr, кик через MMIO WPTR, LINEAR_COPY и FENCE пакеты, GTT→VRAM→GTT в `ferens sdma-copy`
- [x] несколько очередей и VMID, приоритеты — до 8 слотов через CP_HQD (ME1 pipe0-3 и ME2 с mec2.bin), doorbell MEC_RING0+slot, CP_HQD_PIPE/QUEUE_PRIORITY + CP_HQD_QUANTUM, VMID1 со своей таблицей VM_CONTEXT1, GTT-аллокации в обе таблицы, `ferens queues` пускает слот 0 (VMID0, normal) и слот 1 (VMID1, high) разом

## tier 2 — события (готово)

- [x] CP_INT и IH ring через uio — tonga_ih-последовательность (RB_SIZE log2(байт/4), WPTR writeback, overflow-танец, IV 16 байт, rptr в байтах), SDMA trap (src 224) и compute EOP через запись 0x20000000 в CPC_INT_STATUS (src 178), `ferens ih` гоняет оба пути через uio_pci_generic и poll на /dev/uioN
- [x] свой kernel-модуль для MSI — kernel/ferens_irq: PCI-драйвер Polaris-идов, один MSI-вектор, waitqueue-мост на /dev/ferens_irq0 с ioctl WAIT/COUNT/INFO/RESET, вся настройка IH в userspace; RPTR_REARM только под MSI, как в ядре; `ferens msi` гоняет SDMA trap + compute EOP под MSI
- [x] fence по прерыванию вместо поллинга — trait IrqWait (FerensMod ioctl / UioDevice poll), wait_fence_irq с перепроверкой фенса после каждого прерывания, `ferens queues-irq` ждёт фенсы обоих слотов по прерыванию с откатом на uio
- [x] детект зависания CP по таймаутам — hang_report по таймауту в обоих wait-путях: rptr/wptr слота, CP_HQD_ACTIVE и CP_HQD_DEQUEUE_REQUEST через SRBM_GFX_CNTL-мультиплексор, GRBM/SRBM-статусы и VM-фолты; автоматический dequeue-сброс не сделан

## tier 3 — живучесть

- [x] dequeue-сброс зависшего слота — reset_slot повторяет gfx_v8_0_deactivate_hqd(adev, 2): DEQUEUE_REQUEST = 2 под SRBM-мультиплексором, опрос CP_HQD_ACTIVE 200 мс, обнуление DEQUEUE_REQUEST/PQ_RPTR/PQ_WPTR; оба wait-пути по таймауту фенса автоматически пробуют сброс и включают результат в hang-отчёт, `ferens reset` показывает сброс и повторный диспетч на восстановленном слоте
- [x] SMU: управление частотами — SMC-канал сообщений с handshake из smu7_smumgr.c (SMC_RESP_0/SMC_MESSAGE_0/SMC_MSG_ARG_0), проверка smu7_is_smc_ram_running, PPSMC_MSG_API_GetSclk/MclkFrequency в единицах 10 кГц, DPM-таблица из SMC SRAM через заголовок прошивки (SMU7_FIRMWARE_HEADER_LOCATION + офсеты из smu73.h/smu73_discrete.h, выверены компиляцией), форс максимума/минимума через SCLKDPM/MCLKDPM_SetEnabledMask и unforce через PCIeDPM_UnForceLevel с полными масками, `ferens clocks` без отвязки драйвера; записи DPM-таблиц, VddC и thermal остаются за чертой
- [x] GPU soft reset — маски GRBM/SRBM_SOFT_RESET собираются по busy-битам (gfx_v8_0_check_soft_reset + gmc_v8_0_check_soft_reset + busy SDMA из SRBM_STATUS2; в ядре SDMA-биты безусловные, ferens ставит только при busy), pre-фаза gfx_v8_0_pre_soft_reset (rlc stop, CP_ME_CNTL halt, deactivate открытых HQD, MEC halt, cg_off), сброс через GMCON_DEBUG GFX_STALL/GFX_CLEAR и установку/снятие битов с паузами 50 мкс, пост — полный bring_up (golden, RLC/PG, MEC, VM) и тестовый диспетч, `ferens soft-reset`
- [x] реакция на VM fault — диспетч на VA 0 (пустая PTE вне identity- и GTT-окон), волна умирает, CP лочит VM_CONTEXT0_PROTECTION_FAULT_STATUS без прерываний, декод как gmc_v8_0_vm_decode_fault (protections/vmid/страница/RW/ASCII-блок из MCCLIENT), очистка через VM_CONTEXT0_CNTL2, dequeue-сброс слота и корректный диспетч — полный цикл детект/декод/recovery, `ferens fault`; interrupt-путь для VM fault (src_id 223) не делается, фолт ловится поллингом
- [x] suspend/resume — suspend глушит IH/SDMA0/1/GFX/MEC/RLC и CG в порядке amdgpu и сохраняет 256 байт PCI config space в /var/lib/ferens/<slot>.bin (pci_save_state), resume сверяет vendor/device снапшота, восстанавливает config (dwords 15..=4, командный регистр последним — как pci_restore_state) и делает полный bring-up с тестовым диспетчем, `ferens suspend` / `ferens resume`; PCIe D3 не трогается — power state остаётся за ядром
- [x] полный GPU reset через sysfs reset — детекция методов сброса (sysfs reset_method от ядра 5.15+ или обход capability-списка config space: DEVCAP.FLR, AF-капа TP|FLR, PMCSR NO_SOFT_RESET), запись 1 в sysfs reset отдаёт pci_reset_function ядра (save/disable/reset/restore, порядок pci_reset_fn_methods: device_specific/acpi/flr/af_flr/pm/bus), проверка факта сброса канарками — CP_HQD_ACTIVE активной очереди и MEC-халт в CP_MEC_CNTL обязаны обнулиться, затем полный bring-up и тестовый диспетч, `ferens full-reset`; D3cold (ACPI _PR3) и device_specific-квирки остаются за ядром

## tier 4 — интерфейс

- [ ] собственный ABI поверх очередей (char device, ioctl)
- [ ] аллокатор буферов общего вида поверх GTT/VRAM
- [ ] rust-API для клиентов

## про C-код amdgpu

Golden-таблицы, init-последовательности, форматы ucode — это списки MMIO-записей, портируем их дословно, Rust тут ничего не выигрывает. Собственный kernel-модуль ferens_irq на tier 2 стал мостом прерываний (один MSI-вектор и waitqueue); вся остальная работа с устройством остаётся в userspace через sysfs и BAR-mmap.

## известные пропуски

- i2c-твик из polaris10 golden (0x67DF rev 0xc7 с конкретными subsystem id) не портирован — требует atombios i2c
- SRBM_STATUS3 и полная расшифровка всех busy-битов SRBM не нужны для диагностики compute-пути, дамп ограничен GRBM/SRBM_STATUS/SRBM_STATUS2
