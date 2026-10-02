# Ghidra: знания поsetup'у для Boulder Dash NES (macOS, Apple Silicon)

Обновлено: 2026-10-01. Рабочий worktree: `.worktrees/nes-remake` (branch `nes-remake`).

## Компоненты

| Что | Где |
|---|---|
| Ghidra 12.1.4 | `~/prj/tools/ghidra` (zip с GitHub releases, `Ghidra_12.1.4_build`) |
| Ghidra-проект | `~/prj/tools/bd_ghidra/BoulderDash.gpr` |
| venv (pyghidra, capstone, pypdf, pymupdf) | `.worktrees/nes-remake/.venv` |
| Скрипты | `.worktrees/nes-remake/tools/` (`ghidra_query.py`, `GhidraSeedFunctions.java`, `ghidra_seed.py`, `disasm.py`) |
| Ключевой артефакт | `.worktrees/nes-remake/analysis/prg.bin` (32К PRG ROM без iNES-заголовка) |

## Важные особенности установки

### 1. Натив декомпилятора отсутствует в релизе для macOS
В zip только `os/linux_x86_64` и `os/win_x86_64`. Для mac_arm_64 — собрать вручную
(исходники входят в релиз: `Ghidra/Features/Decompiler/src/decompile/cpp`):

```bash
cd ~/prj/tools/ghidra/Ghidra/Features/Decompiler/src/decompile/cpp
# точный список файлов — из build.gradle секции decompile(NativeExecutableSpec)
clang++ -O2 -std=c++20 -I. -c <каждый .cc> -o /tmp/bd_obj/<имя>.o
clang++ -O2 -o decompile /tmp/bd_obj/*.o -lz
mkdir -p ~/prj/tools/ghidra/Ghidra/Features/Decompiler/os/mac_arm_64
cp decompile ~/prj/tools/ghidra/Ghidra/Features/Decompiler/os/mac_arm_64/
```
Всего ~79 файлов (marshal.cc ... xml.cc + ghidra_*.cc + signature*.cc). zlib нужна — ссылаться `-lz`.
`sleigh` собирать НЕ нужно: .sla-файлы прекомпилированы в релизе.

### 2. Java
Работает на Temurin 26 (главное — Ghidra сам стартует, он пишет требования в лог при проблемах).

### 3. Пути с точкой запрещены
Ghidra (ProjectLocator) не открывает проекты в путях, где элемент начинается с `.` —
поэтому проект живёт в `~/prj/tools/bd_ghidra`, а НЕ в `.worktrees/...`.

### 4. Лок-файлы после падений
После креша остаются `~/prj/tools/bd_ghidra/BoulderDash.lock` и `.lock~` → проект
открывается READ-ONLY (симптом: `ReadOnlyException`). Лечение: `rm -f *.lock*`.

## Импорт ROM

```bash
~/prj/tools/ghidra/support/analyzeHeadless ~/prj/tools/bd_ghidra BoulderDash \
  -import analysis/prg.bin \
  -processor 6502:LE:16:default \
  -loader BinaryLoader -loader-baseAddr 0x8000 -cspec default \
  -scriptPath tools \
  -preScript GhidraSeedFunctions.java
```

- `-processor 6502` НЕ работает — нужен полный ID языка `6502:LE:16:default`
- `GhidraSeedFunctions.java` создаёт функции по `analysis/func_seeds.txt`
  (точки входа из capstone-трассы: jsr-цели, вектора, 19 обработчиков состояний
  из таблицы на $A5F2). Без засева — 39 функций, с засевом — 125+.

## Python-скрипты в headless НЕ работают
`analyzeHeadless` не запускает .py (нужно стартовать весь Ghidra в PyGhidra-режиме;
`PYGHIDRA=1` на analyzeHeadless НЕ действует). Решение: Java-скрипты
(GhidraScript, компилируются на лету).

### Гоча scriptPath
Ghidra компилирует ВСЕ .java в scriptPath → посторонние .java (например из клона
GhidraMCP с его AppTest.java) ломают сборку с `package junit.framework does not
exist`. Держать scriptPath чистым.

## pyghidra: рабочий API (версия 3.1.0)

```python
import os
os.environ['GHIDRA_INSTALL_DIR'] = os.path.expanduser('~/prj/tools/ghidra')
import pyghidra
pyghidra.start()
from ghidra.base.project import GhidraProject
pdir = os.path.abspath(os.path.expanduser('~/prj/tools/bd_ghidra'))
gp = GhidraProject.openProject(pdir, 'BoulderDash', False)
prog = gp.openProgram('/', 'prg.bin', False)   # ВАЖНО: третий аргумент False!
# ... изменения в transaction: prog.startTransaction(...)/endTransaction(tx, True)
gp.save(prog)   # сохранение только у mutable-программы
gp.close()
```

Гоча:
- `openProgram(folder, name, True)` открывает IMMUTABLE → `ReadOnlyException:
  Location does not exist for a save operation` на `gp.save`. Нужен `False`.
- `pyghidra.open_program(...)` (старый API) — deprecated и падает на повторном
  импорте; `pyghidra.open_project()` требует абсолютный путь и возвращает сырой
  Java Project (домен-объекты доставать муторно). Юзать `GhidraProject` (utility-класс).
- Создание функции: `fm.createFunction('name', addr, AddressSet(addr, addr),
  SourceType.USER_DEFINED)` — источник именно `USER_DEFINED` (не `USER`),
  body обязателен (иначе `Function body must contain the entrypoint`).
- Анализ из python: `pyghidra.analyze(prog)` (AutoAnalysisManager.startAnalysis
  кидает `NoTransactionException`).
- Переименование: `f.setName(name, SourceType.USER_DEFINED)` внутри transaction.

## Запросы: tools/ghidra_query.py

```
.venv/bin/python tools/ghidra_query.py functions                  # список функций
.venv/bin/python tools/ghidra_query.py decompile <addr>...        # C-псевдокод
.venv/bin/python tools/ghidra_query.py disasm <addr> [n]          # листинг
.venv/bin/python tools/ghidra_query.py xrefs <addr>               # ссылки на адрес
.venv/bin/python tools/ghidra_query.py rename <addr> <name>       # персистентно
.venv/bin/python tools/ghidra_query.py data <addr> <len>          # дамп байт
.venv/bin/python tools/ghidra_query.py batch spec.json            # пачка в JSON
```
Каждый запуск поднимает JVM (~10-15с) — для серий использовать `batch`.

## Известная карта Boulder Dash (пополняемая)

### Машина состояний
- Диспетчер: `$A011`: `lda $1f; asl; tax; lda $a5f2,x → jmp ($11)`. `$1F` — состояние (0-18), `$20` — суб-состояние, у карточки пещеры — вложенное `$70`/`$71`
- `dispatch_tail` ($A4CB): универсальный tail-jump — `pla` вынимает адрес возврата, таблица указателей лежит inline сразу после `jsr $A4CB`, индекс в A. Обработчики не возвращаются (цепочки продолжений)
- `frame_wait` ($A0C9): очистить `$17`, крутиться до NMI; `pad_read` ($A12B): 2 пада с дебаунсом, `$23/$24` — состояние (1=нажата!), `$27/$28` — изменение, `$29/$2A` — фронт нажатия
- Полярность контроллера: **1 = нажата** (D0 $4016); `$9A/$9B` — активный игрок (копия `$23/$29`)

### Пещеры (РАСШИФРОВАНО 2026-10-01)
- **Таблица `$B86C`**: 24 пещеры × 3 байта `(chr_bank, ppu_lo, ppu_hi)`; данные лежат В CHR-ROM
- Миры 1-4 → CHR банки 0-3 ($1800/$19B8/$1B70/$1D28), миры 5-6 → банк 7
- Формат: 440 байт = 22 строки × 20 байт, **ниббл-упаковка** (байт = 2 клетки, старший полубайт левее) → сетка 40×22
- Загрузчик `cave_read_from_chr` ($B7EE→$B818): читает CHR через PPU $2007, разворачивает в RAM **$03E0-$074F** (880 байт), формат ячейки в рантайме: `объект<<4 | флаги`
- Декодер: `tools/extract_caves.py` → `analysis/caves.txt` (все 24)
- Словарь клеток (структурно подтверждён, уточняется): 0=пусто, 2=dirt?, 3=бордер-стена, 5=внутр. стена, 7=грязь(доминанта), 8=объекты стартовой пещеры, 14=выход?

### Ключевые процедуры (переименованы в Ghidra-проекте)
- `nmi_handler` ($A2C7), `audio_nmi_update` ($8006, 4 канала, структуры $0300+), `sub_B007` (NMI: скролл+PPUCTRL+CHR-банки MMC1)
- `stream_render` ($A1BD): протокол экранных стримов — байт=тайл; `$FF`=конец, `$FE`+2б=PPU-адрес, `$FD`=строка+1, `$FC`=toggle PPUCTRL bit2, `$FB n t`=RLE(n×t)
- `vram_to_ram_copy` ($A313): CHR/VRAM→$0600 512 байт (A=банк, X/Y=PPU-адрес)
- `state_card` ($AB7C): карточка города; выход — через `$70`/`$71` (START прокручивает фазы)
- `$C420`: wipe-переходы (таблицы паттернов $D091), `$C944`: текст карточки по `$8D>>2` (названия миров в таблице $C99A)
- `$8D` = индекс пещеры 0-23; `$4D` — счётчик строк сетки (22)

### Эмулятор (tools/emu.py, py65)
CPU 6502 + стабы: VRAM 2КБ (гориз. зеркало, **бит 11**), CHR-ROM 32КБ с банками MMC1,
PPUADDR-латч (сброс чтением $2002), буферизованное чтение $2007, спрайт-0 хит окно,
vblank NMI по PPUCTRL bit7, контроллеры (1=нажата), OAM DMA. Кадр PAL ~33247 циклов (3 цикла/шаг).
Драйвер меню: титл(START 240к) → интро(~700к) → START → пароль(A) → A → выбор города (RIGHT) → A → A → A → карточка (START).

### Геймплей (раскрыто 2026-10-01, сессия 2)
- **STATE 11 ($AB7C) = ГЕЙМПЛЕЙ.** $70/$71 — фазы интро-карточки (START прокручивает);
  при $70=0 без START → `$ABEB` = игровой цикл кадра:
  `jsr $C071 (ввод/диспетчер $91&0xF) → $CDB4/$CE57/$CFCC (физика) → $C04A → $B19D (камера) → $CA53 (рендер)`
- Ввод Рокфорда: `$C0AE/$C156` — каждые 8-й кадр ($FE&7==0); A+B = grab/suicide; `$9A` = удержание, `$9B` = фронты
- Позиция игрока: `$93-$96` (x,y пиксели 16-бит); камера: `$C0/$C1` (X), `$BE/$BF` (Y); цель камеры = игрок
- Рендер: **метатайлы 2×2**; таблица наборов `$F383` (16 слов) — клетка → набор из 6 кадров
  (кадр выбирается флагами анимации; спец-коды $8X/$9X/$AX/$DX/$EX в `$B586`)
- Очередь обновлений экрана `$C7-$FC`: записи по 6 байт (ppu_hi, ppu_lo, 4 тайла), пишется `$B544`, читается `$B0C7`
- Валидатор окна: `$B24B` — грид→`$0106` (окно 16×15), строки грида по указателям `$B37A`

### Словарь клеток (подтверждён поведением+CHR-артом; см. tools/cell_dict.py)
0=BOULDER(валун,круг) · 1=boulder-вариант · 2=SPACE(фон 08/09/18/19) · 3,5=СТЕНЫ(разн.) ·
4=ВЫХОД (выглядит стеной до нормы; ровно 1 на пещеру, у краёв!) · 6=MAGIC WALL (пачками в 1-4/4-4/6-4) ·
7=DIRT · 8=DIAMOND(мерцает) · 9,10,11=ВРАГИ (в статике только 11) · 12=AMOEBA · 13=SPEC(×3 всего) ·
14=TRAIL (след Рокфорда, рантайм) · 15=выход-открытый? (рантайм)

### Метаданные пещер (ЗАКРЫТО 2026-10-01, сессия 3 — см. tools/cave_data.py)
- **`$F233`**: 24 указателя → параметры пещеры (12 байт):
  `[start_row, start_col, ?, ?, norm_A, time_A, norm_B, time_B, norm_C, time_C, norm_D, time_D]`
  (норма алмазов / время сек по вариантам; вариант = `$74&3` = A-D)
- **`$B950`**: 24 указателя → патчи правок грида: `[cnt_C, cnt_D, тройки (row, col, клетка<<4)...]`
  (A/B — базовая карта; C/D — правки: стены-перекрытия, доп. враги $B0, перемещение старта $D0)
- Старт Рокфорда: приоритет — клетка 13 в гриде (поиск $B910: снизу-вверх/справа-налево → $B8=row|C0, $B9=col);
  иначе start_row/col из $F233-структуры → `$93/$95` (клеточные), затем `$BE80`: след $E0 на старт + ×16 (пиксели)
- **`$B1/$B2/$B3` = ВРЕМЯ (BCD: единицы/десятки/сотни)**; dec в `$D03F` (заем 9); аларм при «030» ($D027);
  `$AE/$AF/$B0` = НОРМА алмазов (тоже поразрядно, нач. из norm[variant]); `$7C/$7D` = 2000 = порог extra life
- `$BC98`: 24×9 — интро-пролёт камеры (старт camX/Y + цель + режимы)
- `$C99A`: бонусы очков по мирам (100/125/150/175/200/225, ×2 вызов $CCE8)
- `$B706`: 6 указателей (по мирам) — маршруты курсора town-select

### Шрифт титула (CHR банк 5, фон из окна $1000+, PPUCTRL bit4)
E=$19 R=$1A D=$1B T=$1C S=$1D P=$1E L=$1F A=$23 C=$24 O=$26 F=$29 I=$2A W=$2B N=$2C V=$2D
'.'=$2F H=$37 B=$4E M=$6E G=$6F U=$7F (алфавит НЕ ASCII-порядок!)
Тексты титула: «BOULDER DASH IS A REGISTERED TRADEMARK», «PUBLISHED UNDER LICENSE FROM FIRST STAR SOFTWARE INC.»
Названия миров на экране выбора (state 6) — ГРАФИЧЕСКИЕ тайлы $01-$07 + $2E/$2D (крупный шрифт) — расшифровка по артам pending

### Прочее
- Пещерные данные в CHR: это объясняет «странные» тайлы в chr_bank*_ascii.txt
- `BOULDER DASH PAL` — $7FF0; тексты в тайл-кодах $7657+; таблица 13 указателей $7638 → $F666+
- Опрос джойпадов $4016/$4017 — $A139-$A14B
