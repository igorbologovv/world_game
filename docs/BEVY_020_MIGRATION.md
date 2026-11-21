# Миграция на Bevy 0.20 — статус

**Статус: WIP / не компилируется гарантированно.** Рабочая игра остается в `feature/pond-depth-waves`. Эта ветка создана для поэтапной миграции, НЕ для слияния в production.

## Выполнено

- Обновлены workspace-зависимости `bevy`, `bevy_ecs`, `bevy_math` до `0.20.0` в `Cargo.toml`.
- Экранная статистика FPS и F8-диагностика леса переведены с `TextBundle` / `Style` на `Text`, `TextFont`, `TextColor`, `Node`.
- В нескольких игровых крейтах заменены устаревшие вызовы `get_single[_mut]`, `delta_seconds`, `elapsed_seconds` на новые API. Эти изменения требуют проверки компилятором.

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
