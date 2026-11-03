# Проверка оригинального Horizon 3 ноября 2026

Проверен `horizon-rust/horizon`, текущий `main`/HEAD: `f42ef42ef6ceccb3c6a20bea2481d87949a51c81`.
Подтверждение: `git ls-remote` и GitHub connector. Последняя включённая в форк база upstream — `953fbd73c60dda8cdfd2fdeb951717065fa9414d`; GitHub compare показывает **1485 новых коммитов**, без отставания оригинала от этой базы.

Сравнение: https://github.com/horizon-rust/horizon/compare/953fbd73c60dda8cdfd2fdeb951717065fa9414d...f42ef42ef6ceccb3c6a20bea2481d87949a51c81

GitHub compare вернул только первые 300 файлов. Это ограниченный обзор и точечная проверка интеграции, а не полный аудит 1485 коммитов и не утверждение совместимости с новым HEAD.

Прочитаны оригинальные `native-hooks.json`, `game/native_hooks.rs`, `game/lan_actions.rs`, `game/saving.rs`, `game/commands.rs`, `vanillagen.rs`, `horizon-mods/src/api_level.rs`, `Cargo.toml` на указанном HEAD.

* Native Hooks API по-прежнему **359**. В каталогах оригинала и нашей базы по 386 hooks и 113 helpers; добавленных имён нет. Это сравнение имён, не доказательство равенства поведения и контрактов всех функций. В исходнике upstream есть новые ещё не пронумерованные маркеры `@API:...@`.
* `as_guest_messaged`, `as_guest` и `for_each_player` сохранены. Нашего `for_each_player_messaged` в оригинале нет: при будущей синхронизации нужно сохранить доставку фоновых сообщений Civilization клиентам.
* Сохраняются `VanillaTerrain::find_closest_biome`, `ChunkGenerator::worker` с `GeneratorWorker`, `Game::place_state`. Оригинальный `teleport_to` остаётся приватным; форку нужна видимость `pub(super)` для отдельного модуля старта.
* Owned NPC API 1 и система Civilization — дополнения форка; в оригинале файлов `horizon-mods/src/owned_npcs.rs` нет. Их жизненный цикл и сохранения необходимо сохранить при переносе.
* В оригинале изменены сохранения, LAN и множество клиентских файлов; в первых 300 файлах также есть изменения Cargo.lock, Cargo.toml, рендера, streaming и локализации. Полную синхронизацию следует делать отдельно с проверкой конфликтов, сохранений и сетевых сценариев.

Оригинальный репозиторий, удалённые ветки и текущая база форка не изменялись. Эта проверка не является merge, публикацией или обновлением пользовательской установки.
