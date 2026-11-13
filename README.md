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

Небольшая поправка к исходной постановке: RX 550 - это Polaris 12 (Lexa), а «Polaris 21» в открытых таблицах ядра не существует. RX 580 - это Polaris 10 (Ellesmere). Обе карты из плана поддержаны.

## Что уже работает

Скан PCI идёт через /sys/bus/pci/devices: читаются vendor, device, revision, распарсены BAR с флагами памяти. Регистровый BAR мапится через mmap, чтение и запись volatile. Любой регистр доступен через пару MM_INDEX и MM_DATA, в байтах это смещения 0x0000 и 0x0004.

Конструктор PM4-пакетов собирает NOP, SET_SH_REG, SET_CONTEXT_REG, DISPATCH_DIRECT, INDIRECT_BUFFER, INDIRECT_BUFFER_CONST, WRITE_DATA, WAIT_REG_MEM и EVENT_WRITE. Заголовки type-3. У SET_SH_REG смещение регистра стоит первым словом тела. У DISPATCH_DIRECT порядок такой: dim_x, dim_y, dim_z, dispatch_initiator. Командное кольцо переносит пакеты через границу буфера и заполняет хвост однодвордными NOP.

Compute-очередь ставится через KFD, то есть через /dev/kfd и ioctl AMDKFD. Порядок такой: топология читается из /sys/class/kfd/kfd/topology/nodes, оттуда берутся gpu_id, PCI-адрес и минор render-ноды. GTT-память выделяется через ALLOC_MEMORY_OF_GPU и MAP_MEMORY_TO_GPU, хост-мапа делается mmap'ом render-ноды на тот же виртуальный адрес, что ушел в GPU, так что CPU VA и GPU VA совпадают. CREATE_QUEUE получает кольцо, страницу указателей и EOP; CP пишет read pointer в память через RPTR_REPORT_ADDR, а забор write pointer'а идет через WPTR_POLL_ADDR, поэтому и тот и другой лежат в нашей аллокации. На gfx8 doorbell - это 4 байта, слот очереди внутри среза процесса равен queue_id * 4, значение - write pointer в двордах. Завершение ловится фенсом: WRITE_DATA с WR_CONFIRM пишет магию в GTT, хост крутится на read_u32 с таймаутом.

Сам диспетч собирает такой поток пакетов: SET_SH_REG на COMPUTE_PGM_LO/HI (адрес кернела в единицах 256 байт), COMPUTE_PGM_RSRC1 и COMPUTE_PGM_RSRC2, COMPUTE_NUM_THREAD_X/Y/Z, COMPUTE_USER_DATA_0/1 (указатель на кернарги), затем DISPATCH_DIRECT и WRITE_DATA-фенс. Кернел - это 10 двордов рукописного ISA для gfx8: s_load_dwordx4 тянет буфер-дескриптор из кернаргов, buffer_store_dword пишет магию в память, s_endpgm. Дескриптор собран по таблице Buffer Resource из ISA-мануала GCN3: база 48 бит, stride в битах 61:48, num_records в третьем дворде, третье слово нулевое (TYPE=0 - buffer).

У CLI четыре подкоманды. `ferens list` показывает найденные AMD-карты. `ferens show 01:00.0` читает CONFIG_MEMSIZE, нужен root. `ferens pm4-dump` печатает hexdump того же потока пакетов, что уходит в очередь, но офлайн - удобно сверять с ядром. `ferens dispatch` прогоняет демо-кернел на живой карте через KFD: печатает версию интерфейса, gpu_id, номер очереди, потом ждет фенс и показывает, что кернел записал в память.

Требования для dispatch: загруженный amdgpu с amdkfd (это штатный путь ядра, своего модуля ferens не ставит), доступ к /dev/kfd и к render-ноде - обычно хватает группы video или render, root не обязателен. Ядра новее 4.20. Тестов 44, среди них сверка заголовков пакетов с эталонными словами из ядра и сверка каждого слова ISA-кернела с энкодингами из LLVM-тестов. CI гоняет fmt, clippy с -D warnings и тесты.

## Чего нет

Очередь без KFD: прямое программирование CP_HQD через MMIO, чтобы DISPATCH работал без загруженного amdgpu вообще. Это следующий шаг, сейчас очередь ходит через штатный amdkfd. Дальше по списку: менеджер VRAM, прерывания, управление частотами, несколько очередей и приоритеты.

MMIO без root падает с EPERM, это ожидаемо: файлы resourceN в sysfs открывает только root.

## Структура

- ferens-core - PCI-скан, mmap BAR, MMIO через index/data
- ferens-gfx8 - регистры gfx8, коды PM4, конструктор пакетов, кольцо, ISA-кернел, сборка диспетча
- ferens-kfd - /dev/kfd: ioctl, топология, GTT-аллокатор, compute-очередь, doorbell
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
- [vid.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/vid.h)
- [gfx_v8_0.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/gfx_v8_0.c)
- [kfd_ioctl.h](https://github.com/torvalds/linux/blob/master/include/uapi/linux/kfd_ioctl.h)
- [kfd_chardev.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_chardev.c)
- [kfd_mqd_manager_vi.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_mqd_manager_vi.c)
- [LLVM MC-тесты AMDGPU](https://github.com/llvm/llvm-project/tree/main/llvm/test/MC/AMDGPU)
- [GCN3 ISA manual](https://github.com/tpn/pdfs/blob/master/AMD%20-%20GCN3%20Instruction%20Set%20Architecture%20-%20Graphics%20Core%20Next%20Architecture%2C%20Generation%203%20(Revision%201.0%2C%20March%202015).pdf)
- [ROCT-Thunk-Interface](https://github.com/ROCm/ROCT-Thunk-Interface)
- [открытая документация AMD](https://www.amd.com/en/developer/resources/developer-guides-manuals.html)

## Лицензия

MIT, текст в [LICENSE](LICENSE).
