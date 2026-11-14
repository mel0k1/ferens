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

- [ ] GPU reset: SRBM/GRBM soft reset, восстановление golden regs, полный ресетап очередей
- [ ] реакция на VM fault: стоп очереди, decode, recovery
- [ ] suspend/resume

## tier 4 — интерфейс

- [ ] собственный ABI поверх очередей (char device, ioctl)
- [ ] аллокатор буферов общего вида поверх GTT/VRAM
- [ ] rust-API для клиентов

## про C-код amdgpu

Golden-таблицы, init-последовательности, форматы ucode — это списки MMIO-записей, портируем их дословно, Rust тут ничего не выигрывает. Собственный kernel-модуль ferens_irq на tier 2 стал мостом прерываний (один MSI-вектор и waitqueue); вся остальная работа с устройством остаётся в userspace через sysfs и BAR-mmap.

## известные пропуски

- i2c-твик из polaris10 golden (0x67DF rev 0xc7 с конкретными subsystem id) не портирован — требует atombios i2c
- SRBM_STATUS3 и полная расшифровка всех busy-битов SRBM не нужны для диагностики compute-пути, дамп ограничен GRBM/SRBM_STATUS/SRBM_STATUS2
