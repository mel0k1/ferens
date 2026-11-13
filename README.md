# ferens

[![ci](https://github.com/mel0k1/ferens/actions/workflows/ci.yml/badge.svg)](https://github.com/mel0k1/ferens/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

Userspace-драйвер для видеокарт AMD поколения GCN4 на Rust. Идея такая: без своего модуля ядра, только sysfs, mmap и прямая запись в регистры Polaris. Документация AMD открыта, исходники ядра и Mesa тоже, поэтому каждая константа в коде сверена с реальными заголовками, а не придумана.

## Железо

| Кристалл | Карты | PCI ID из amdgpu_drv.c |
|----------|-------|------------------------|
| Polaris 10 (Ellesmere) | RX 470, 480, 570, 580, 590 | 0x67C0-0x67DF, 0x6FDF |
| Polaris 11 (Baffin) | RX 460, 560 | 0x67E0-0x67FF |
| Polaris 12 (Lexa) | RX 540, 550 | 0x6980-0x699F |
| VegaM | Radeon RX Vega M | 0x694C, 0x694E, 0x694F |

## Что уже работает

Скан PCI идёт через /sys/bus/pci/devices: читаются vendor, device, revision, распарсены BAR с флагами памяти. Регистровый BAR - наименьший memory-BAR (на Polaris это BAR2 на 256 КБ), апертура VRAM - наибольший; оба мапятся через mmap. Регистры доступны двумя путями: косвенно через пару MM_INDEX и MM_DATA (в байтах 0x0000 и 0x0004) и напрямую записью в BAR2 по смещению регистр*4 - второй путь и использует amdgpu, им же ходит compute-часть.

Конструктор PM4-пакетов собирает NOP, SET_SH_REG, SET_CONTEXT_REG, DISPATCH_DIRECT, INDIRECT_BUFFER, INDIRECT_BUFFER_CONST, WRITE_DATA, WAIT_REG_MEM и EVENT_WRITE. Заголовки type-3. У SET_SH_REG смещение регистра стоит первым словом тела. У DISPATCH_DIRECT порядок такой: dim_x, dim_y, dim_z, dispatch_initiator. Командное кольцо переносит пакеты через границу буфера и заполняет хвост однодвордными NOP.

Compute-очередь ставится через KFD, то есть через /dev/kfd и ioctl AMDKFD. Порядок такой: топология читается из /sys/class/kfd/kfd/topology/nodes, оттуда берутся gpu_id, PCI-адрес и минор render-ноды. GTT-память выделяется через ALLOC_MEMORY_OF_GPU и MAP_MEMORY_TO_GPU, хост-мапа делается mmap'ом render-ноды на тот же виртуальный адрес, что ушел в GPU, так что CPU VA и GPU VA совпадают. CREATE_QUEUE получает кольцо, страницу указателей и EOP; CP пишет read pointer в память через RPTR_REPORT_ADDR, а забор write pointer'а идет через WPTR_POLL_ADDR, поэтому и тот и другой лежат в нашей аллокации. На gfx8 doorbell - это 4 байта, слот очереди внутри среза процесса равен queue_id * 4, значение - write pointer в двордах. Завершение ловится фенсом: WRITE_DATA с WR_CONFIRM пишет магию в GTT, хост крутится на read_u32 с таймаутом.

Compute-очередь работает и без KFD - напрямую через регистры CP_HQD в MMIO, свой крейт ferens-mmio. Драйвер сам грузит микрокод MEC из /lib/firmware/amdgpu (polaris10/11/12_mec.bin, mec2 опционально): halt через CP_MEC_CNTL, слова в CP_MEC_ME1_UCODE_DATA под адресом из CP_MEC_ME1_UCODE_ADDR, в конце - запись версии ucode в UCODE_ADDR, потом halt снимается. HQD слота ME1 pipe0 queue0 выбирается через SRBM_GFX_CNTL (PIPEID [1:0], MEID [3:2], VMID [7:4], QUEUEID [10:8]) и программируется в том же порядке, что делает mqd_commit в gfx_v8_0.c: база PQ в единицах 256 байт, RPTR-репорт и WPTR-poll в байтах, CP_HQD_PQ_CONTROL с размером кольца (order_base_2 от двордов минус один), RPTR_BLOCK_SIZE 5, PRIV_STATE и KMD_QUEUE, квантум с EN/SCALE/DURATION, PRELOAD_SIZE 0x53 в persistent state, в конце CP_HQD_ACTIVE = 1. Активация KIQ-стилем, без MAP_QUEUES и без HWS: очередь сразу исполняет PM4.

Память очереди - VRAM, без аллокатора и без GTT: кольцо, MQD-тень, таблица страниц, страница меты (rptr-репорт, wptr, кернарги, фенс, вывод) и ISA-кернел разложены в BAR0-апертуре начиная с 64 МБ, чтобы не задеть vBIOS-фреймбуфер; CPU пишет туда же через mmap этой апертуры. VMID 0 получает собственную плоскую таблицу страниц: 4096 PTE с identity-отображением окна VRAM лежат в самой VRAM, VM_CONTEXT0_PAGE_TABLE_BASE_ADDR/START/END задают диапазон, PAGE_TABLE_DEPTH = 0 делает таблицу плоской, инвалимация через VM_L2_CNTL2 и VM_INVALIDATE_REQUEST с чтением RESPONSE. Write pointer доставляется дверным звонком, как у compute ring 0 в amdgpu: CP_MEC_DOORBELL_RANGE_LOWER/UPPER и CP_PQ_STATUS.DOORBELL_ENABLE, сама запись - 32-битный writel в BAR2 по смещению doorbell_index * 4, слот 0x10 (MEC_RING0). Поток диспетча перед настройкой кернела ставит COMPUTE_STATIC_THREAD_MGMT_SE0-3 в 0xFFFFFFFF - у только что запрограммированной очереди CU не разрешены иначе. Завершение ловится тем же WRITE_DATA-фенсом в VRAM.

Сам диспетч собирает такой поток пакетов: SET_SH_REG на COMPUTE_PGM_LO/HI (адрес кернела в единицах 256 байт), COMPUTE_PGM_RSRC1 и COMPUTE_PGM_RSRC2, COMPUTE_NUM_THREAD_X/Y/Z, COMPUTE_USER_DATA_0/1 (указатель на кернарги), затем DISPATCH_DIRECT и WRITE_DATA-фенс. Кернел - это 10 двордов рукописного ISA для gfx8: s_load_dwordx4 тянет буфер-дескриптор из кернаргов, buffer_store_dword пишет магию в память, s_endpgm. Дескриптор собран по таблице Buffer Resource из ISA-мануала GCN3: база 48 бит, stride в битах 61:48, num_records в третьем дворде, третье слово нулевое (TYPE=0 - buffer).

У CLI пять подкоманд. `ferens list` показывает найденные AMD-карты. `ferens show 01:00.0` читает CONFIG_MEMSIZE, нужен root. `ferens pm4-dump` печатает hexdump того же потока пакетов, что уходит в очередь, но офлайн - удобно сверять с ядром. `ferens dispatch` прогоняет демо-кернел на живой карте через KFD: печатает версию интерфейса, gpu_id, номер очереди, потом ждет фенс и показывает, что кернел записал в память. `ferens dispatch-mmio` делает то же самое без /dev/kfd: сам грузит mec-микрокод, строит таблицы страниц VMID0, программирует CP_HQD и запускает кернел через собственный дверной звонок.

Требования для dispatch: загруженный amdgpu с amdkfd (это штатный путь ядра, своего модуля ferens не ставит), доступ к /dev/kfd и к render-ноде - обычно хватает группы video или render, root не обязателен. Ядра новее 4.20. Требования для dispatch-mmio жёстче: root (BAR в sysfs открывается только им), amdgpu не должен держать карту - драйвер видит биндинг через симлинк driver и отказывается работать, и в /lib/firmware/amdgpu должны лежать mec-файлы под чип. Тестов 70, среди них сверка заголовков пакетов с эталонными словами из ядра, сверка каждого слова ISA-кернела с энкодингами из LLVM-тестов, сверка слов HQD, таблицы страниц и разбора заголовка микрокода. CI гоняет fmt, clippy с -D warnings и тесты.

## Чего нет

RLC: MEC самостоятелен, RLC-микрокод не грузится - CP jump table на Polaris не нужен (в amdgpu он создается только для Carrizo и Stoney). Менеджер VRAM: раскладка очереди в апертуре фиксированная, аллокатора нет. GTT и VMID больше нуля: очередь ходит только по VRAM через identity-окно VMID 0, системная память через таблицы не мапится. Прерывания: завершение ловится фенсом, IH не подключен. Дальше по списку: управление частотами, несколько очередей и приоритеты.

MMIO без root падает с EPERM, это ожидаемо: файлы resourceN в sysfs открывает только root.

## Структура

- ferens-core - PCI-скан, mmap BAR, MMIO через index/data и напрямую
- ferens-gfx8 - регистры gfx8, коды PM4, конструктор пакетов, кольцо, ISA-кернел, сборка диспетча
- ferens-kfd - /dev/kfd: ioctl, топология, GTT-аллокатор, compute-очередь, doorbell
- ferens-mmio - MEC-микрокод, CP_HQD через MMIO, таблицы VMID0, compute-очередь в VRAM
- ferens - CLI

## Сборка

Linux обязателен, код под unix-only. Компилятор закреплён в rust-toolchain.toml на версии 1.100.0.

```
cargo build --release
cargo test --all
```

## Откуда константы

Все значения выверены по открытым исходникам:

- gfx_8_0_d.h - регистры compute-блока и GRBM_GFX_INDEX
- gfx_8_0_sh_mask.h - раскладка полей COMPUTE_PGM_RSRC1/2: VGPRS [5:0], SGPRS [9:6], USER_SGPR [5:1] в rsrc2
- vid.h - коды PM4-пакетов и макрос PACKET3
- bif_5_0_d.h - MM_INDEX, MM_DATA, CONFIG_MEMSIZE
- gfx_v8_0.c - раскладка тел пакетов и порядок слов
- gfx_v8_0.c v4.14 - загрузка mec.bin: halt через CP_MEC_CNTL, CP_MEC_ME1/ME2_UCODE_ADDR/DATA, запись версии ucode в UCODE_ADDR; версия между 4.14 и 5.4 из amdgpu исчезла, поэтому ссылка на тег v4.14
- amdgpu_amdkfd_gfx_v8.c - выбор HQD-слота через SRBM_GFX_CNTL (PIPEID/MEID/VMID/QUEUEID), hqd_is_occupied сверяет CP_HQD_PQ_BASE с адресом в единицах 256 байт
- gfx_v8_0.c mqd_init и mqd_commit - значения CP_HQD_PQ_CONTROL (QUEUE_SIZE = order_base_2(ring/4) - 1, RPTR_BLOCK_SIZE, UNORD_DISPATCH 0, PRIV_STATE 1, KMD_QUEUE 1), CP_HQD_PERSISTENT_STATE PRELOAD_SIZE 0x53, квантум EN/SCALE=1/DURATION=10, CP_HQD_IB_CONTROL MIN_IB_AVAIL_SIZE 3 и MTYPE 3, порядок записей и deactivate_hqd через DEQUEUE_REQUEST
- oss_3_0_d.h и oss_3_0_sh_mask.h - SRBM_GFX_CNTL 0x391: PIPEID [1:0], MEID [3:2], VMID [7:4], QUEUEID [10:8]
- gfx_8_0_d.h CP_HQD-блок - смещения CP_MQD_BASE_ADDR 0x3245 ... CP_HQD_ERROR 0x3278, CP_MEC_CNTL 0x208D, CP_MEC_ME1/ME2_UCODE 0xF81A-0xF81D, CP_MEC_DOORBELL_RANGE 0x305C/0x305D, CP_PQ_STATUS 0x30B8, CP_PQ_WPTR_POLL_CNTL 0x3083
- amdgpu_ucode.h - заголовок gfx_firmware_header_v1_0: ucode_size_bytes на 0x14, ucode_array_offset_bytes на 0x18, ucode_feature_version/jt_offset/jt_size после общего блока
- amdgpu_doorbell_mgr.c и amdgpu_doorbell.h - doorbell на CIK/VI это 32-битный writel в BAR2 по индексу dword (doorbell.base = pci_resource_start(pdev, 2)), слоты KIQ 0x000, MEC_RING0 0x010, MAX 0x3FF; gfx_v8_0.c ставит CP_MEC_DOORBELL_RANGE_LOWER/UPPER как индекс << 2 и CP_PQ_STATUS.DOORBELL_ENABLE
- amdgpu_ttm.c, amdgpu_gmc.c и amdgpu_vm.h - PTE 64-битный: адрес в битах 63:12, флаги VALID/SYSTEM/SNOOPED/EXECUTABLE/READABLE/WRITEABLE, для VRAM берутся VALID|SNOOPED|EXECUTABLE|READABLE|WRITEABLE
- gmc_v8_0.c - конфиг VM_CONTEXT0: MC_VM_MX_L1_TLB_CNTL (SYSTEM_ACCESS_MODE 3), VM_L2_CNTL/CNTL2/CNTL3/CNTL4, PAGE_TABLE_DEPTH 0 - плоская таблица, START/END/BASE_ADDR в страницах, инвалимация VM_INVALIDATE_REQUEST с чтением VM_INVALIDATE_RESPONSE; fragment_size = 4
- gmc_8_1_d.h и gmc_8_1_sh_mask.h - смещения VM-регистров Polaris: VM_L2_CNTL 0x500, VM_CONTEXT0_CNTL 0x504, VM_INVALIDATE 0x51E/0x51F, PAGE_TABLE 0x54F/0x557/0x55F, MC_VM_MX_L1_TLB_CNTL 0x819
- linux-firmware - polaris10_mec.bin, polaris10_mec2.bin (и polaris11/polaris12 аналогично)
- kfd_ioctl.h - номера ioctl, структуры, QUEUE_TYPE, флаги аллокаций
- kfd_chardev.c и kfd_queue.c - требования к кольцу и страницам указателей: wptr и rptr должны лежать в мапе на одну GPU-страницу, кольцо - в аллокации размера PAGE_ALIGN(ring_size)
- kfd_mqd_manager_vi.c - WPTR в двордах для PM4-очередей, RPTR_BLOCK_SIZE = 5, кодировка cp_hqd_pq_control
- kfd_device_queue_manager.c - на pre-SOC15 doorbell_id равен queue_id
- kfd_doorbell.c и kfd_priv.h - срез doorbell'ов на процесс, кодировка mmap-офсета: биты 63:62 - тип, 61:46 - gpu_id
- GCN3 ISA manual, таблица Buffer Resource Descriptor - формат буфер-дескриптора
- LLVM test/MC/AMDGPU - эталонные байты ISA-энкодингов, каждое слово кернела сверено с ними
- ROCT-Thunk-Interface src/queues.c - эталонный клиент create_queue: мапа doorbell'ов, units write pointer'а

Ссылки:

- [gfx_8_0_d.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/gca/gfx_8_0_d.h)
- [gfx_8_0_sh_mask.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/gca/gfx_8_0_sh_mask.h)
- [oss_3_0_d.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/oss/oss_3_0_d.h)
- [oss_3_0_sh_mask.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/oss/oss_3_0_sh_mask.h)
- [gmc_8_1_d.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/gmc/gmc_8_1_d.h)
- [gmc_8_1_sh_mask.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/gmc/gmc_8_1_sh_mask.h)
- [vid.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/vid.h)
- [gfx_v8_0.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/gfx_v8_0.c)
- [gfx_v8_0.c v4.14, загрузка MEC-микрокода](https://github.com/torvalds/linux/blob/v4.14/drivers/gpu/drm/amd/amdgpu/gfx_v8_0.c)
- [amdgpu_amdkfd_gfx_v8.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_amdkfd_gfx_v8.c)
- [amdgpu_ucode.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_ucode.h)
- [amdgpu_doorbell_mgr.c](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/amd/amdgpu/amdgpu_doorbell_mgr.c)
- [amdgpu_doorbell.h](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/amd/amdgpu/amdgpu_doorbell.h)
- [amdgpu_ttm.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_ttm.c)
- [amdgpu_gmc.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_gmc.c)
- [amdgpu_vm.h](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/amd/amdgpu/amdgpu_vm.h)
- [gmc_v8_0.c](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/amd/amdgpu/gmc_v8_0.c)
- [kfd_ioctl.h](https://github.com/torvalds/linux/blob/master/include/uapi/linux/kfd_ioctl.h)
- [kfd_chardev.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_chardev.c)
- [kfd_mqd_manager_vi.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_mqd_manager_vi.c)
- [LLVM MC-тесты AMDGPU](https://github.com/llvm/llvm-project/tree/main/llvm/test/MC/AMDGPU)
- [GCN3 ISA manual](https://github.com/tpn/pdfs/blob/master/AMD%20-%20GCN3%20Instruction%20Set%20Architecture%20-%20Graphics%20Core%20Next%20Architecture%2C%20Generation%203%20(Revision%201.0%2C%20March%202015).pdf)
- [ROCT-Thunk-Interface](https://github.com/ROCm/ROCT-Thunk-Interface)
- [открытая документация AMD](https://www.amd.com/en/developer/resources/developer-guides-manuals.html)

## Лицензия

MIT, текст в [LICENSE](LICENSE).
