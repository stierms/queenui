/**
 * The single import site for the IPC contract.
 *
 * Everything in the app imports types from `../types`, never from
 * `../types/models.gen` directly, so the generated module can be re-emitted
 * (`cargo test --manifest-path src-tauri/Cargo.toml tests::generate_frontend_ipc_models`)
 * without any call site caring.
 *
 * Three layers, in this order:
 *   - `./models.gen` — generated from the Rust types by ts-rs. The wire truth.
 *     Never edited by hand; a name here always wins.
 *   - `./models` — only what the generator cannot express: closed unions over
 *     `string` wire fields, UI-only shapes, and `Omit`-derived request shapes.
 *     It declares no name the generated module declares, so `export *` is
 *     unambiguous and a future generated `CampaignStatus` collides loudly
 *     instead of silently shadowing.
 *   - `./helpers` — ours: snapshot lookups, `assertNever`, and the narrowing
 *     guards that turn a generated `string` into one of those closed unions.
 */
export * from "./models.gen";
export * from "./models";
export * from "./helpers";
