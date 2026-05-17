
## Handoff Log — Filter Message Tags Toggle (2026-04-10)

### Summary
Added a user-facing toggle in Advanced Settings to filter out 'eof by user' and 'interrupted by user' tags from chat output. This allows users to hide or show these technical end-of-generation markers as needed.

### Steps Taken
1. Added a "Filter message tags" checkbox to the Advanced Settings GUI (`frontend/advanced.html`).
2. Extended the backend settings (`AppSettings` in `src-tauri/src/main.rs`) to persist the `filter_msg_tags` boolean, defaulting to true.
3. Updated Tauri commands to support reading and updating this setting.
4. Modified the frontend chat streaming handler (`frontend/main.js`) to suppress these tag messages when the toggle is enabled.
5. The toggle can be changed at runtime and will take effect immediately for new messages.

### Follow-up fix
6. Broadened frontend tag detection to catch EOF variations (e.g. `EOF by user`, `End of input`, `interrupted`) in `frontend/main.js` to ensure those markers are also hidden when the filter is enabled.

### Robustness update
7. Improved detection to strip leading punctuation (like `>`) and match additional EOF/end phrases so messages like `> EOF by user` are correctly filtered when the toggle is enabled.

---
Change implemented by GitHub Copilot on 2026-04-10.

### Summary
Fixed an issue where the llama model would only reply once, then all subsequent inferences were immediately cancelled with an "interrupted by user" message, even when the user did not press stop.

### Steps Taken
1. Identified that the cancel flag (AtomicBool) was not being reset before each inference, causing stale cancellation to persist across requests.
2. Located the inference start in `cmd_send_message` in `src-tauri/src/main.rs`.
3. Added `instance.cancel.store(false, Ordering::Relaxed);` before each call to `run_inference`.
4. Verified that this ensures only explicit user stops will cancel inference, and normal chat flow is uninterrupted.

---
Change implemented by GitHub Copilot on 2026-04-10.

### Small cleanup
Renamed unused parameter `instance` to `_instance` in `crates/adaptive_llama/src/process.rs` to silence an unused-variable compiler warning. This is a deliberate, non-functional change to reduce build noise.

Change implemented by GitHub Copilot on 2026-04-11.

### Context extension feature
Added `Max Context` select to the main Settings UI and wired persistence/backend support; fixed a JavaScript syntax error in `frontend/main.js` that prevented the renderer from loading. Feature implemented and syntax corrected — ready for user testing.

Change implemented by GitHub Copilot on 2026-04-11.