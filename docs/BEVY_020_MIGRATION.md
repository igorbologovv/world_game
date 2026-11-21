## Runtime GPU validation — 21 November 2026

On RADV RX 6750 XT, `cargo run -p world-client` starts successfully and runs at ~219 FPS, but only sky/background and UI render. The runtime reports `expected global item ... found '#'` in water and terrain/vegetation shader modules: the project still contained naga_oil `#import` directives. The log also reports `No standalone woodland tree prefabs found` (independent missing game assets).

The migration branch now contains nine `.wesl` versions of its custom shaders and all `Material::vertex_shader()/fragment_shader()` paths point to `.wesl`. Changes include WESL `import`, `@if`, new Bevy PBR `bevy_pbr::render::*` shader packages, `constants::MATERIAL_BIND_GROUP`, decompressed Bevy 0.20 mesh vertex input, updated PBR alpha-discard call signatures, and `view.world_position.xyz`. Old `.wgsl` files are retained as an unreferenced baseline until runtime shaders are verified.

**Not yet validated:** WESL import resolution, shader pipeline compilation, PBR lighting or visible spherical terrain. `cargo check` does not validate runtime shaders. Re-run the game on the GPU, capture ALL shader errors (not only `#import`) and inspect water, terrain, forest. Do not call the migration complete just because the app starts.

# Миграция на Bevy 0.20 — статус

**Статус: WIP / не компилируется гарантированно.** Рабочая игра остается в `feature/pond-depth-waves`. Эта ветка создана для поэтапной миграции, НЕ для слияния в production.

## Выполнено

- Обновлены workspace-зависимости `bevy`, `bevy_ecs`, `bevy_math` до `0.20.0` в `Cargo.toml`.
- Экранная статистика FPS и F8-диагностика леса переведены с `TextBundle` / `Style` на `Text`, `TextFont`, `TextColor`, `Node`.
- В нескольких игровых крейтах заменены устаревшие вызовы `get_single[_mut]`, `delta_seconds`, `elapsed_seconds` на новые API. Эти изменения требуют проверки компилятором.

## Требования к Rust

Bevy 0.20.1 и `wesl` требуют **rustc не ниже 1.97.1**. В ветке добавлен `rust-toolchain.toml` с `channel = "1.97.1"`. В отличие от простого `bevy = "0.20.0"`, Cargo может разрешить патч-релиз 0.20.1.

На машине разработчика с rustup:

```sh
rustup toolchain install 1.97.1
cd ~/world-bevy20
rustc --version
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo check -p world-water
```

Для Arch Linux с системным Rust вместо rustup нужно обновить `rust` через системный менеджер пакетов (с учётом свободного места на Btrfs). Миграцию не следует тестировать на Rust 1.96.0.

## Обновление: отказ от старого bevy_terrain (21 ноября 2026)

После подтверждения 55 несовместимостей в `bevy_terrain-020` миграционная игра теперь напрямую зависит от `big_space 0.14.0-rc.0` — без старого terrain-рендерера. У проекта уже имеется собственный рендерер поверхности (`world-terrain`). В `world-player` добавлена независимая debug-камера. Перенесены основные UI, mesh bundles, input, и импорты ресурсов на API 0.20.

**Новая точка проверки**: `cargo check -p world-client` из `~/world-bevy20`; эта ветка ещё НЕ проверена компилятором и не подтверждена визуально.

Внимание: ранний скрипт `tools/setup_bevy20_terrain.sh` сейчас **не нужен** и не должен запускаться вновь. Он локально менял `Cargo.toml` на путь к `../bevy_terrain-020`. Перед `git pull` убедиться, что это единственная локальная правка манифеста, и сохранить её через `git stash push -- Cargo.toml`, если требуется. Оригинальный `~/bevy_terrain` и отдельный `~/bevy_terrain-020` не менять и не удалять автоматически.

Ключевая инженерная проверка после компиляции: новейший `big_space` использует `Grid`, `CellCoord`, `CellTransform`; адаптер `ReferenceFrames` для **одной планеты** пока намеренно возвращает корневой `Grid`. Проверить положение камеры, центр координат, близкие/дальние объекты, нормали, лесной streaming и planet horizon. До этого нельзя считать миграцию завершённой.

## Подтверждено компилятором (21 ноября 2026)

При `cargo check -p world-water` первый сбой — `world-core`:
- `SpatialBundle` и `set_parent()` заменены на `Transform + Visibility` и `set_parent_in_place()`.
- `BigSpacePlugin<i32>` реализует `Plugins` из `bevy_app 0.14.2`, а приложение ожидает `bevy_app 0.20.1`.
- `spawn_big_space` не доступен на типе `Commands` из Bevy 0.20, потому что расширение импортировано из сборки Bevy 0.14.

**Следующее условие для продолжения:** выделить независимую копию `~/bevy_terrain` для миграции, обновить её зависимости на Bevy 0.20, портировать `big_space` и другие модули до успешного `cargo check`. Нельзя менять оригинал на месте, иначе сломается рабочая версия на 0.14.

Локальная проверка источника зависимости:

```sh
git -C ~/bevy_terrain status --short
git -C ~/bevy_terrain remote -v
grep -nE '^bevy([[:space:]]|_)|^big_space|^version' ~/bevy_terrain/Cargo.toml
cd ~/world-bevy20
cargo tree -i bevy_app@0.14.2
```

## Изолированная копия terrain — подтверждённая причина и следующий этап

`cargo tree -i bevy_app@0.14.2` показал точную цепочку: `world_game -> ../bevy_terrain -> bevy 0.14.2 / big_space 0.7.0`. Локальный `~/bevy_terrain` также имеет изменённый `examples/spherical.rs` и неотслеживаемые `assets/terrains/`, `assets/textures/`; **оригинал не менять и не чистить**.

Для перехода существует `big_space 0.14.0-rc.0`, рассчитанный на API Bevy 0.20 (исходный Cargo.toml зависит от `bevy_* 0.20.0-rc.2`). Однако API `big_space` существенно изменился с 0.7: `BigSpacePlugin`, `ReferenceFrame`, `GridCell` и `GridTransform` нельзя механически перенести — нужно отдельно мигрировать `bevy_terrain/src/big_space.rs` и пользовательские системы позиционирования.

Для безопасного начала:

```sh
cd ~/world-bevy20
git pull --ff-only origin migration/bevy-0.20
bash tools/setup_bevy20_terrain.sh
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo check -p world-core
```

Скрипт создаёт `~/bevy_terrain-020` как отдельный Git worktree, указывает **только миграционную** игру на него и обновляет версии зависимостей этой копии. Ожидаются ошибки несовместимых API. Из-за прежних Btrfs ENOSPC и двух версий Bevy не запускать полную сборку до локализации ошибок `bevy_terrain`; не удалять общую папку `target`.

## Критические блокеры

1. **Локальная зависимость `../bevy_terrain`.** Исходники этого отдельного проекта отсутствуют в репозитории. Плагин предоставляет `big_space::{GridTransform, ReferenceFrame, ReferenceFrames, BigSpacePlugin, GridCell}`, `DebugCameraController`, собственные системы floating origin и terrain. Его надо портировать и компилировать с Bevy 0.20 *раньше* перехода остальных крейтов. Просто изменить номер версии нельзя.
2. **Удалённые bundle-типы**: `PbrBundle`, `MaterialMeshBundle`, `SpatialBundle`, а также старые UI bundles. Нужен переход на required components (`Mesh3d`, `MeshMaterial3d<T>`, `Transform`, `Visibility`), в том числе в коде streaming, LOD и рендеринга планеты.
3. **Кастомный рендеринг.** В `world-terrain`, `world-vegetation`, `world-water` используются устаревшие пути модулей Bevy для `Mesh`, `Indices`, `RenderAssetUsages`, `Aabb` и материалов. Также нужно перенести WGSL imports и внутренние структуры рендера на API 0.20.
4. **Камера, UI и glTF.** Проверить `Camera3d`, `Projection`, события/сообщения ввода, ассеты изображений, регистрации system sets и обширный код `world-player`.
5. **`Cargo.lock` ещё от старой версии.** Перегенерировать только после успешного разрешения всех зависимостей в совместимом workspace. Не коммитить lockfile с неразрешёнными версиями.

## Необходимая проверка на машине разработчика

Сначала восстановить доступ к `../bevy_terrain` с исходниками и перенести **его** на 0.20. После этого:

```sh
cd ~/world-woodland
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --workspace
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo run -p world-client
```

После перехода нужна проверка Vulkan/RADV: floating origin на планете, рельеф, перемещение игрока, деревья (включая импортированные LOD), океан/реки/озёра, материал и маски воды, UI, FPS. Без этого результат нельзя назвать законченной миграцией.

## Источники миграции

- [Bevy 0.14 → 0.15](https://bevy.org/learn/migration-guides/0-14-to-0-15/)
- [Bevy 0.15 → 0.16](https://bevy.org/learn/migration-guides/0-15-to-0-16/)
- [Bevy 0.16 → 0.17](https://bevy.org/learn/migration-guides/0-16-to-0-17/)
- [Bevy 0.17 → 0.18](https://bevy.org/learn/migration-guides/0-17-to-0-18/)
- [Bevy 0.18 → 0.19](https://bevy.org/learn/migration-guides/0-18-to-0-19/)
- [Bevy 0.19 → 0.20](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
