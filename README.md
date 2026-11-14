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

RLC-микрокод грузится перед MEC тем же прямым способом, что и в gfx_v8_0_rlc_resume из v4.14 - в современных ядрах прямой путь у RLC исчез так же, как у MEC: его забрал SMU, поэтому референс тот же тег. Порядок такой: RLC выключается через RLC_CNTL.RLC_ENABLE_F32, serdes-юниты дожидаются простоя опросом RLC_SERDES_CU_MASTER_BUSY по каждому из четырех SE (polaris10/11/12 в amdgpu имеют max_shader_engines = 4) через GRBM_GFX_INDEX и дальше RLC_SERDES_NONCU_MASTER_BUSY по маске SE/GC/TC0/TC1; затем отключение clock и power gating (CGCG_EN/CGLS_EN в RLC_CGCG_CGLS_CTRL и RLC_CGCG_CGLS_CTRL_3D, RLC_PG_CNTL = 0), сброс через GRBM_SOFT_RESET.SOFT_RESET_RLC с паузой 50 мкс на фазу, для polaris11/12 - init_pg: clear state, save/restore-список и задержки PG (об этом дальше), слова микрокода пишутся в RLC_GPM_UCODE_DATA под адресом 0 из RLC_GPM_UCODE_ADDR, в конце в UCODE_ADDR уходит версия ucode и RLC_ENABLE_F32 = 1. Файлы - polaris10/11/12_rlc.bin; их заголовок - rlc_firmware_header_v2_0 с мажором 2, размером 104 байта и массивом ucode с 0x100, поэтому RLC-парсер принимает и major 1, и major 2 - общий блок заголовка у них совпадает. destroy гасит RLC последним, как gfx_v8_0_hw_fini, а перед ним возвращает clock gating в исходное состояние.


Память очереди - VRAM, без аллокатора: кольцо, MQD-тень, таблица страниц, блок clear state, страница меты (rptr-репорт, wptr, кернарги, фенс, вывод) и ISA-кернел разложены в BAR0-апертуре начиная с 64 МБ, чтобы не задеть vBIOS-фреймбуфер; CPU пишет туда же через mmap этой апертуры. Раскладка: кольцо 0x4000000, MQD-тень 0x4001000, таблица страниц 0x4002000 (64 КБ под 8192 PTE), clear state 0x4012000, мета 0x4014000, кернел 0x4015000, default page 0x4016000. VMID 0 получает собственную плоскую таблицу страниц: окно разбито на две половины по 16 МБ - identity-отображение VRAM [0x4000000, 0x5000000) и GTT [0x5000000, 0x6000000); VM_CONTEXT0_PAGE_TABLE_BASE_ADDR/START/END задают диапазон (START 0x4000, END 0x5FFF), PAGE_TABLE_DEPTH = 0 делает таблицу плоской, инвалимация через VM_L2_CNTL2 и VM_INVALIDATE_REQUEST с чтением RESPONSE. Write pointer доставляется дверным звонком, как у compute ring 0 в amdgpu: CP_MEC_DOORBELL_RANGE_LOWER/UPPER и CP_PQ_STATUS.DOORBELL_ENABLE, сама запись - 32-битный writel в BAR2 по смещению doorbell_index * 4, слот 0x10 (MEC_RING0). Поток диспетча перед настройкой кернела ставит COMPUTE_STATIC_THREAD_MGMT_SE0-3 в 0xFFFFFFFF - у только что запрограммированной очереди CU не разрешены иначе. Завершение ловится тем же WRITE_DATA-фенсом.

GTT - системная память, видимая GPU через ту же таблицу VMID 0. `ferens dispatch-gtt` аллоцирует страницу хоста: mmap с MAP_LOCKED и MAP_POPULATE, mlock, затем PFN каждой 4К-страницы читается из /proc/self/pagemap (нужен root, иначе PFN занулены). PTE системной страницы отличается от VRAM-ного битом SYSTEM (бит 1): VALID|SYSTEM|SNOOPED|READABLE|WRITEABLE против VALID|SNOOPED|EXECUTABLE|READABLE|WRITEABLE у identity-страниц - кодировка из amdgpu_vm.h. PTE пишутся прямо в таблицу в VRAM через апертуру, после чего VMID 0 инвалидируется. Кернарги, фенс и вывод живут в этой хост-странице: CPU кладёт буфер-дескриптор в свою память, CP пишет магию фенса и вывод по GTT-адресам, хост опрашивает фенс и читает результат у себя - ни одного чтения из BAR в цикле ожидания.

RLC-часть ведёт себя как PG-хост: внутри rlc-резюма, между сбросом и загрузкой микрокода, для polaris11/12 выполняется init_pg из gfx_v8_0.c - clear state, save/restore-список и задержки. CSB собирается из таблицы clearstate_vi.h (7 экстентов, 885 двордов значений, обёрнутых в PREAMBLE_CNTL/CONTEXT_CONTROL/SET_CONTEXT_REG/SET_CONTEXT_REG с PA_SC_RASTER_CONFIG/1 прочитанными из живых регистров/CLEAR_STATE - те же пакеты, что собирает get_csb_buffer) и кладётся в VRAM, RLC_CSIB_ADDR_LO/HI и RLC_CSIB_LENGTH на неё указывают. Save/restore-список берётся из самого rlc.bin: у заголовка v2 на 0x38-0x54 лежат reg_restore_list_size, reg_list_format_start, starting_offsets_start, размеры и смещения двух массивов; формат (первая часть - direct-список, RLC_FormatDirectRegListLength = 14 двордов) прогоняется через копию gfx_v8_0_parse_ind_reg_list с дедупликацией индексов, поток значений уходит в RLC_SRM_ARAM_DATA под RLC_SRM_ARAM_ADDR = 0, формат - в RLC_GPM_SCRATCH_DATA, туда же - размер restore-списка (дворды пополам) и десять стартовых смещений, уникальные индексы пакуются в пары RLC_SRM_INDEX_CNTL_ADDR_x (младшие 18 бит) и RLC_SRM_INDEX_CNTL_DATA_x (старшие 12). Дальше RLC_SRM_CNTL получает AUTO_INCR_ADDR и SRM_ENABLE, а задержки PG - RLC_PG_DELAY по 0x10 во все четыре поля, RLC_PG_DELAY_2.SERDES_CMD_DELAY = 3, RLC_AUTO_PG_CTRL.GRBM_REG_SAVE_GFX_IDLE_THRESHOLD = 0x55f0 и CP_RB_WPTR_POLL_CNTL.IDLE_POLL_COUNT = 0x60. У polaris10 этот путь в ядре пропущен (init_pg перечисляет только CZ/Stoney и P11/P12/VEGAM), поэтому у RX 580 ferens его тоже не делает - pg_initialized честно говорит false.

Поверх запущенного RLC `ferens gate` включает coarse grain clock gating: в RLC_CGTT_MGCG_OVERRIDE снимаются биты CGCG (3) и CGLS (4), serdes-юниты получают BPM-команды через RLC_SERDES_WR_CU/NONCU_MASTER_MASK = 0xFFFFFFFF и RLC_SERDES_WR_CTRL (cmd в битах 25:16, номер BPM-регистра в 31:28, BPM_ADDR 0xff и RSVD_BPM_ADDR - кодировка gfx_v8_0_send_serdes_cmd): сначала Clear CGCG_OVERRIDE (BPM-регистр 2), потом Set CGLS_EN (регистр 0); между командами serdes дожидается простоя по всем четырём SE. Финал - RLC_CGCG_CGLS_CTRL: золотое значение Polaris 0x0001003c (REP_COMPANSAT_DELAY 0xf, порог простоя 0x100) плюс биты CGCG_EN|CGLS_EN, итого 0x0001003f, у 3D-регистра - та же задержка и порог без EN-битов (его EN-биты в ядре не включают - там 3D-CGCG идёт через SMU), и CP_INT_CNTL_RING0 получает биты CNTX_BUSY/CNTX_EMPTY/CMP_BUSY/GFX_IDLE (18-21), которыми RLC узнаёт о простое графики. `ferens gate pg` дополнительно ставит в RLC_PG_CNTL биты STATIC_PER_CU_PG (3) и DYN_PER_CU_PG (2) - статическое и динамическое power gating CU-островов руками RLC. В amdgpu на Polaris всё это ходит сообщениями в SMU (gfx_v8_0_polaris_update_gfx_clock_gating), pg_flags стоят в ноль и до RLC-битов дело не доходит; ferens делает ровно тот регистровый путь, которым тот же код включает CGCG на Fiji/Carrizo - интерфейс у RLC один и тот же. destroy перед остановкой RLC гасит CG обратно (cg_off: override-биты возвращаются, четыре чтения CB_CGTT_SCLK_CTRL будят графику, обратные BPM-команды Set CGCG_OVERRIDE и Clear CGLS_EN, EN-биты снимаются).

Сам диспетч собирает такой поток пакетов: SET_SH_REG на COMPUTE_PGM_LO/HI (адрес кернела в единицах 256 байт), COMPUTE_PGM_RSRC1 и COMPUTE_PGM_RSRC2, COMPUTE_NUM_THREAD_X/Y/Z, COMPUTE_USER_DATA_0/1 (указатель на кернарги), затем DISPATCH_DIRECT и WRITE_DATA-фенс. Кернел - это 10 двордов рукописного ISA для gfx8: s_load_dwordx4 тянет буфер-дескриптор из кернаргов, buffer_store_dword пишет магию в память, s_endpgm. Дескриптор собран по таблице Buffer Resource из ISA-мануала GCN3: база 48 бит, stride в битах 61:48, num_records в третьем дворде, третье слово нулевое (TYPE=0 - buffer).

У CLI семь подкоманд. `ferens list` показывает найденные AMD-карты. `ferens show 01:00.0` читает CONFIG_MEMSIZE, нужен root. `ferens pm4-dump` печатает hexdump того же потока пакетов, что уходит в очередь, но офлайн - удобно сверять с ядром. `ferens dispatch` прогоняет демо-кернел на живой карте через KFD: печатает версию интерфейса, gpu_id, номер очереди, потом ждет фенс и показывает, что кернел записал в память. `ferens dispatch-mmio` делает то же самое без /dev/kfd: сам грузит rlc- и mec-микрокод, строит таблицы страниц VMID0, программирует CP_HQD и запускает кернел через собственный дверной звонок. `ferens gate [pg]` включает CGCG/CGLS (с `pg` - ещё и power gating) поверх уже загруженного RLC и печатает RLC_CGCG_CGLS_CTRL и RLC_PG_CNTL до и после. `ferens dispatch-gtt` - диспетч с кернаргами и выводом в системной памяти через GTT.

Требования для dispatch: загруженный amdgpu с amdkfd (это штатный путь ядра, своего модуля ferens не ставит), доступ к /dev/kfd и к render-ноде - обычно хватает группы video или render, root не обязателен. Ядра новее 4.20. Требования для dispatch-mmio, gate и dispatch-gtt жёстче: root (BAR в sysfs открывается только им, PFN из pagemap - тоже), amdgpu не должен держать карту - драйвер видит биндинг через симлинк driver и отказывается работать, и в /lib/firmware/amdgpu должны лежать rlc- и mec-файлы под чип. Тестов 104, среди них сверка заголовков пакетов с эталонными словами из ядра, сверка каждого слова ISA-кернела с энкодингами из LLVM-тестов, сверка слов HQD, таблицы страниц, разбора заголовков микрокода и полной последовательности записей RLC-резюма и init_pg в порядке ядра. CI гоняет fmt, clippy с -D warnings и тесты.

## Чего нет

RLC: jump table и cp table не программируются - в ядре они создаются только на Carrizo/Stoney. MGCG/MGLS (medium grain) и light sleep не трогаются: на Polaris amdgpu гонит их через SMU-сообщения, своего SMU-канала у ferens нет, включён только coarse grain и RLC-биты PG; само rail-переключение питания остаётся за SMU. GTT - фиксированное окно 16 МБ и VRAM-раскладка без аллокатора общего вида; VMID больше нуля не используются, у каждого процесса была бы своя таблица. Прерывания: завершение ловится фенсом, IH не подключен. Дальше по списку: управление частотами, несколько очередей и приоритеты.

MMIO без root падает с EPERM, это ожидаемо: файлы resourceN в sysfs открывает только root.

## Структура

- ferens-core - PCI-скан, mmap BAR, MMIO через index/data и напрямую
- ferens-gfx8 - регистры gfx8, коды PM4, конструктор пакетов, кольцо, ISA-кернел, сборка диспетча
- ferens-kfd - /dev/kfd: ioctl, топология, GTT-аллокатор, compute-очередь, doorbell
- ferens-mmio - MEC- и RLC-микрокод, CP_HQD через MMIO, таблицы VMID0 с GTT, clear state и SRM, CGCG/PG, compute-очередь в VRAM
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
- gfx_v8_0.c v4.14, RLC-резюм - gfx_v8_0_rlc_stop/reset/load_microcode/start: выключение через RLC_ENABLE_F32, serdes-обход по четырем SE через GRBM_GFX_INDEX, паузы 50 мкс, RLC_GPM_UCODE_ADDR = 0, поток двордов в RLC_GPM_UCODE_DATA, версия в UCODE_ADDR; max_shader_engines = 4 для polaris10/11/12 - это число SE в serdes-обходе; прямой путь у RLC и MEC в современных ядрах отнял SMU, поэтому референс тот же тег
- gfx_8_0_d.h RLC-блок - смещения RLC_CNTL 0xEC00, RLC_PG_CNTL 0xEC43, RLC_SERDES_CU/NONCU_MASTER_BUSY 0xEC61/0xEC62, RLC_CGCG_CGLS_CTRL 0xEC49 и _3D 0xEC9D, RLC_GPM_UCODE_ADDR/DATA 0xF83C/0xF83D, GRBM_SOFT_RESET 0x2008, GRBM_GFX_INDEX 0xC200
- gfx_8_0_d.h, CGCG/PG-регистры - RLC_CGTT_MGCG_OVERRIDE 0xEC48, RLC_SERDES_WR_CU/NONCU_MASTER_MASK 0xEC5D/0xEC5E, RLC_SERDES_WR_CTRL 0xEC5F, RLC_GPM_SCRATCH_ADDR/DATA 0xEC6C/0xEC6D, RLC_SRM_CNTL 0xEC80, RLC_SRM_ARAM_ADDR/DATA 0xEC83/0xEC84, RLC_SRM_INDEX_CNTL_ADDR_0-3 0xEC8B-0xEC8E, RLC_SRM_INDEX_CNTL_DATA_0-3 0xEC93-0xEC96, RLC_PG_DELAY 0xEC4D, RLC_PG_DELAY_2 0xEC1F, RLC_AUTO_PG_CTRL 0xEC55, RLC_CSIB_ADDR_LO/HI/LENGTH 0xECA2-0xECA4, CP_RB_WPTR_POLL_CNTL 0x21C2, CP_INT_CNTL_RING0 0x306A, CB_CGTT_SCLK_CTRL 0xF0A8, PA_SC_RASTER_CONFIG/1 0xA0D4/0xA0D5
- gfx_8_0_sh_mask.h, поля CGCG/PG - RLC_CGCG_CGLS_CTRL CGCG_EN 0x1 / CGLS_EN 0x2, RLC_PG_CNTL DYN_PER_CU 0x4 / STATIC_PER_CU 0x8 / QUICK_PG 0x100000, RLC_SRM_CNTL SRM_ENABLE 0x1 / AUTO_INCR_ADDR 0x2, RLC_PG_DELAY четыре поля по 0xFF, RLC_PG_DELAY_2 SERDES_CMD_DELAY 0xFF00, RLC_AUTO_PG_CTRL GRBM_REG_SAVE_GFX_IDLE_THRESHOLD 0x7FFF8, CP_RB_WPTR_POLL_CNTL IDLE_POLL_COUNT 0xFFFF0000, CP_INT_CNTL_RING0 CMP_BUSY 0x40000 / CNTX_BUSY 0x80000 / CNTX_EMPTY 0x100000 / GFX_IDLE 0x200000, поля RLC_SERDES_WR_CTRL (BPM_DATA 25:16, REG_ADDR 31:28, BPM_ADDR 7:0, RSVD_BPM_ADDR 27)
- gfx_v8_0.c defines - RLC_CGTT_MGCG_OVERRIDE__MGCG 0x4 / CGCG 0x8 / CGLS 0x10, BPM_REG_CGLS_EN 0 / BPM_REG_CGCG_OVERRIDE 2 / BPM_REG_MGCG_OVERRIDE 3, SET_BPM_SERDES_CMD 1 / CLE_BPM_SERDES_CMD 0, RLC_FormatDirectRegListLength 14
- gfx_v8_0.c v6.6 - init_pg (ветки CZ/Stoney и P11/P12/VEGAM), init_save_restore_list и parse_ind_reg_list, init_power_gating (IDLE_POLL_COUNT 0x60, PG_DELAY 0x10x4, SERDES_CMD_DELAY 3, порог 0x55f0), get_csb_buffer (пакеты clear state), get_csb_size, send_serdes_cmd, update_coarse_grain_clock_gating (порядок override → serdes → BPM → ctrl → прерывания), set_powergating_state (P11/P12: STATIC/DYN/QUICK bits), set_clockgating_state (Polaris уходит в SMU-сообщения)
- gfx_v8_0.c v4.14 - порядок rlc_resume: stop, disable CG (CGCG_CGLS_CTRL и _3D &= ~0x3 у Polaris), RLC_PG_CNTL = 0, reset, init_pg, load_microcode, start; референс для загрузки ucode и раскладки резюма
- golden-таблицы polaris10/11/12 из gfx_v8_0.c - mmRLC_CGCG_CGLS_CTRL = 0x0001003c и mmRLC_CGCG_CGLS_CTRL_3D = 0x0001003c
- vi.c - cg_flags Polaris: GFX_MGCG, GFX_RLC_LS, GFX_CP_LS, GFX_CGCG, GFX_CGLS, GFX_3D_CGCG, GFX_3D_CGLS; pg_flags = 0 у всех трёх Polaris
- clearstate_vi.h - таблица clear state: 7 экстентов vi_SECT_CONTEXT_def_1-7 с базами 0xa000/0xa0d6/0xa1f5/0xa200/0xa2a0/0xa2a3/0xa2a5 и 885 двордов значений, один SECT_CONTEXT-секция
- amdgpu_ucode.h, rlc_firmware_header_v2_0 - reg_restore_list_size 0x38, reg_list_format_start 0x3C, starting_offsets_start 0x44, reg_list_format_size_bytes 0x48, reg_list_format_array_offset 0x4C, reg_list_size_bytes 0x50, reg_list_array_offset 0x54
- vid.h - PACKET3_CLEAR_STATE 0x12, PACKET3_CONTEXT_CONTROL 0x28, PACKET3_PREAMBLE_CNTL 0x4A, PACKET3_PREAMBLE_BEGIN/END_CLEAR_STATE 1/0, PACKET3_SET_CONTEXT_REG 0x69, PACKET3_SET_CONTEXT_REG_START 0xa000
- gfx_8_0_sh_mask.h - RLC_CNTL__RLC_ENABLE_F32 бит 0, GRBM_SOFT_RESET__SOFT_RESET_RLC бит 2, GRBM_GFX_INDEX: SE_INDEX [23:16] и broadcast-биты SH 29 / INSTANCE 30 / SE 31, маска занятости NONCU SE|GC|TC0|TC1 = 0xDFFFF
- amdgpu_ucode.h, rlc_firmware_header_v1_0/v2_0 - общий блок одинаковый, ucode_feature_version на 0x20 у обоих; реальные polaris10/12_rlc.bin идут с major 2, header 104 байта, массив ucode с 0x100 - парсер проверен на файлах из linux-firmware
- amdgpu_amdkfd_gfx_v8.c - выбор HQD-слота через SRBM_GFX_CNTL (PIPEID/MEID/VMID/QUEUEID), hqd_is_occupied сверяет CP_HQD_PQ_BASE с адресом в единицах 256 байт
- gfx_v8_0.c mqd_init и mqd_commit - значения CP_HQD_PQ_CONTROL (QUEUE_SIZE = order_base_2(ring/4) - 1, RPTR_BLOCK_SIZE, UNORD_DISPATCH 0, PRIV_STATE 1, KMD_QUEUE 1), CP_HQD_PERSISTENT_STATE PRELOAD_SIZE 0x53, квантум EN/SCALE=1/DURATION=10, CP_HQD_IB_CONTROL MIN_IB_AVAIL_SIZE 3 и MTYPE 3, порядок записей и deactivate_hqd через DEQUEUE_REQUEST
- oss_3_0_d.h и oss_3_0_sh_mask.h - SRBM_GFX_CNTL 0x391: PIPEID [1:0], MEID [3:2], VMID [7:4], QUEUEID [10:8]
- gfx_8_0_d.h CP_HQD-блок - смещения CP_MQD_BASE_ADDR 0x3245 ... CP_HQD_ERROR 0x3278, CP_MEC_CNTL 0x208D, CP_MEC_ME1/ME2_UCODE 0xF81A-0xF81D, CP_MEC_DOORBELL_RANGE 0x305C/0x305D, CP_PQ_STATUS 0x30B8, CP_PQ_WPTR_POLL_CNTL 0x3083
- amdgpu_ucode.h - заголовок gfx_firmware_header_v1_0: ucode_size_bytes на 0x14, ucode_array_offset_bytes на 0x18, ucode_feature_version/jt_offset/jt_size после общего блока
- amdgpu_doorbell_mgr.c и amdgpu_doorbell.h - doorbell на CIK/VI это 32-битный writel в BAR2 по индексу dword (doorbell.base = pci_resource_start(pdev, 2)), слоты KIQ 0x000, MEC_RING0 0x010, MAX 0x3FF; gfx_v8_0.c ставит CP_MEC_DOORBELL_RANGE_LOWER/UPPER как индекс << 2 и CP_PQ_STATUS.DOORBELL_ENABLE
- amdgpu_ttm.c, amdgpu_vm.h и gmc_v8_0.c - PTE 64-битный: адрес в битах 63:12, флаги VALID/SYSTEM/SNOOPED/EXECUTABLE/READABLE/WRITEABLE, SYSTEM = бит 1; для VRAM-страниц берутся VALID|SNOOPED|EXECUTABLE|READABLE|WRITEABLE, для системных страниц GTT - VALID|SYSTEM|SNOOPED|READABLE|WRITEABLE
- gmc_v8_0.c - конфиг VM_CONTEXT0: MC_VM_MX_L1_TLB_CNTL (SYSTEM_ACCESS_MODE 3), VM_L2_CNTL/CNTL2/CNTL3/CNTL4, PAGE_TABLE_DEPTH 0 - плоская таблица, START/END/BASE_ADDR в страницах, инвалимация VM_INVALIDATE_REQUEST с чтением VM_INVALIDATE_RESPONSE; fragment_size = 4
- gmc_8_1_d.h и gmc_8_1_sh_mask.h - смещения VM-регистров Polaris: VM_L2_CNTL 0x500, VM_CONTEXT0_CNTL 0x504, VM_INVALIDATE 0x51E/0x51F, PAGE_TABLE 0x54F/0x557/0x55F, MC_VM_MX_L1_TLB_CNTL 0x819
- linux-firmware - polaris10_mec.bin, polaris10_mec2.bin, polaris10_rlc.bin (и polaris11/polaris12 аналогично)
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
- [clearstate_vi.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/clearstate_vi.h)
- [vi.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/vi.c)
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
