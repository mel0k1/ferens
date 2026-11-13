# ferens

[![ci](https://github.com/mel0k1/ferens/actions/workflows/ci.yml/badge.svg)](https://github.com/mel0k1/ferens/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

Userspace-драйвер для видеокарт AMD поколения GCN4 на Rust. Идея такая: без amdgpu и без своего модуля ядра, только sysfs, mmap и прямая запись в регистры Polaris. Документация AMD открыта, исходники ядра и Mesa тоже, поэтому каждая константа в коде сверена с реальными заголовками, а не придумана.

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

У CLI три подкоманды. `ferens list` показывает найденные AMD-карты. `ferens show 01:00.0` читает CONFIG_MEMSIZE, нужен root. `ferens pm4-dump` печатает hexdump набора пакетов под compute-диспетч.

Тестов 22, среди них сверка заголовков пакетов с эталонными словами из ядра. CI гоняет fmt, clippy с -D warnings и тесты.

## Чего нет

Подача команд в живую очередь. На gfx8 очередь ставится через CP_HQD-регистры либо через KFD, эта часть ещё не написана, так что DISPATCH_DIRECT в карточку пока никто не отправлял. Дальше по списку: менеджер VRAM, загрузка шейдеров, прерывания, управление частотами.

MMIO без root падает с EPERM, это ожидаемо: файлы resourceN в sysfs открывает только root.

## Структура

- ferens-core - PCI-скан, mmap BAR, MMIO через index/data
- ferens-gfx8 - регистры gfx8, коды PM4, конструктор пакетов, кольцо
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
- vid.h - коды PM4-пакетов и макрос PACKET3
- bif_5_0_d.h - MM_INDEX, MM_DATA, CONFIG_MEMSIZE
- gfx_v8_0.c - раскладка тел пакетов и порядок слов

Ссылки:

- [gfx_8_0_d.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/gca/gfx_8_0_d.h)
- [vid.h](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/vid.h)
- [gfx_v8_0.c](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/gfx_v8_0.c)
- [sid.h в Mesa](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/src/amd/common/sid.h)
- [открытая документация AMD](https://www.amd.com/en/developer/resources/developer-guides-manuals.html)

## Лицензия

MIT, текст в [LICENSE](LICENSE).
