# Open validation issues

## CLIENT-LINT-001 — inherited strict Clippy debt (open, quarantined)

The 2026-10-28 protocol-777 cutover and the known-green full-suite baseline
`f5ac9a4` both report **428 library / 452 library-test errors** under:

```sh
cargo clippy --release -p solaris-client --all-targets --message-format=json -- -D warnings
```

Both commands used the same toolchain/build-support environment and the required
`mem-run.sh -m 12G` envelope. The complete multiset of **(lint code, primary source
path)** is identical: 973 primary spans, zero additions and zero removals.
New placement-test module ordering and sign-decoder needless-borrow diagnostics
were corrected rather than added to this quarantine.

Exact local receipts are `.analysis/native263/clippy-baseline.jsonl`,
`.analysis/native263/final-lint-correction.log` and
`.analysis/native263/clippy-final-comparison.json`. Existing failures include
`unused_imports`, `dead_code`, `private_interfaces` and Clippy diagnostics; none
are suppressed or declared fixed. Earlier `65a7f76` comparison receipts remain
historical evidence in `.analysis/clippy-base.log` and `.analysis/clippy.log`.
Strict Clippy is **not green**.

Parity-polish checkpoint (2026-10-30, base `0b00d98`): the required debug
`cargo clippy -p solaris-client --all-targets --message-format=json -- -D warnings`
still fails with **419 library / 444 library-test errors**. No primary diagnostic
intersects the added or changed parity-polish portions; this is not a green
strict-lint result and no warnings were suppressed. Exact output and the
changed-range classification are in `.analysis/parity-polish/clippy-final.jsonl`
and `.analysis/parity-polish/clippy-summary.json`.

RC4 comparison (2026-11-03): clean RC3 `b73105c` with debug workspace
`cargo clippy --workspace --all-targets --message-format=json -- -D warnings`
reported 863 coded error occurrences across library and library-test targets.
The updated candidate stops at its library with 368 errors (raw run
`artifact://776`). Comparing each primary span with added lines in
`git diff --unified=0 b73105c` finds zero overlapping diagnostics. This does not
cover library-test diagnostics, because compilation stops at the library.
Strict Clippy remains failing; the changed-line result does not quarantine or
resolve the remaining debt. Detailed classification is retained at
`.analysis/rc4-client-release/clippy-changed-spans.json`.

The later changed-function audit uses `-D clippy::all --cap-lints warn` to
reach both library and test targets despite the remaining debt. It found 533
Clippy diagnostic occurrences, including repeated diagnostics across targets,
and no diagnostics in functions or impl items changed since `b73105c`
(`.analysis/rc4-client-release/clippy-touched-functions.json`; raw
`.analysis/rc4-client-release/clippy-all-targets-capped.jsonl`). The cap is for
collecting findings, not a passing strict
Clippy gate: the uncapped command still fails. No new Clippy suppressions were
introduced.

## CLIENT-TEST-001 — historical graphics-preset assertion (superseded)

The earlier `65a7f76` receipt recorded
`ui::state::client_settings_tests::cycles_wrap_and_clamp` failing because the
assertion expected `"medium"` after advancing the default Medium preset.
The current complete workspace run passes that same test and all ordinary
tests: **627 client-library + 6 CLI + 90 launcher + 1 path test**, zero failures.
The two ignored client tests are the existing
`motion::tests::bench_motion_cost` and the explicitly external-data
`protocol::packet_ids::tests::all_packet_ids_match_26_3_report`.
The latter was run separately against the explicit local 26.3 report: one
passing test, all 255 packet IDs checked. No assertion was weakened or skipped
to obtain the ordinary suite result. Receipt:
`.analysis/native263/final-acceptance.log`; final sign-decoder closure cleanup
additionally passes all five component regressions in
`.analysis/native263/final-lint-correction.log`.

## CLIENT-GPU-001 — default-driver parallel GPU-test SIGSEGV (open)

The earlier validation history already recorded a parallel GPU-suite SIGSEGV
before this cutover. The reported new SIGSEGV is retained verbatim in
`.analysis/native263/historical-gpu-segv.log`. This is classified as a
pre-existing/environment-dependent GPU-test failure, not a demonstrated new
protocol regression or a claimed driver fix. The exact driver/concurrency
cause remains unproven.

The complete current workspace suite passes with
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.json` (receipt above).
The generic GPU implementation/dependencies were unchanged against `f5ac9a4`;
protocol-specific inline painting presentation is separately regression-tested.
This driver-qualified green result does not establish every backend or physical
display scanout. Windowed lifecycle evidence uses an isolated Xvfb/lavapipe
surface and is labeled accordingly.

## CLIENT-INV-001 — Shift-double-click bulk quick move (open)

The RC.1 review's blanket claim that vanilla moves other stacks after a first
Shift-click empties the clicked slot was incorrect for the pinned 26.3 client.
`AbstractContainerScreen.mouseClicked` replaces `lastQuickMoved` when the
cursor is empty, even when the second Shift-down sees an empty slot;
`mouseReleased` skips the bulk loop when that value is empty. If the second
down sees a nonempty stack with an empty cursor, vanilla instead scans
matching stacks in the released slot's backing container and
quick-moves them in menu order. This remains distinct from `PICKUP_ALL`.
The `shift_double_click_*` regressions document both cases; real-client,
authoritative server-state and Java-oracle scenario verification remain open.

