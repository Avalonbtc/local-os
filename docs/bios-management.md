# BIOS management

The standalone `/bios` page selects a machine; `/bios/:id` links directly to its editor. Machine details also link to it. The page reads cached data immediately and explicitly queues SUM reads; it does not poll the BMC on every page visit.

Install the official Linux x86_64 Supermicro SUM distribution under `tools/sum/` on the controller (the executable must be `tools/sum/sum`). The native service reads it through `RIGDECK_SUM_PATH` (set to `<repo>/tools/sum/sum` by `scripts/native/install.sh`); it must be executable by the service user. SUM is not redistributed in this repository and the application never generates or activates product keys. Machines require a configured BMC password and appropriate existing license.

`GET /api/v1/machines/{id}/bios` returns the cached snapshot. Use the standard jobs API for a single machine:

* `{"kind":"bios_read"}` exports current BIOS settings and queries the license.
* `{"kind":"bios_write","revision":"<snapshot revision>","changes":{"<setting id>":"Disabled"}}` submits only the selected changes. Setting IDs and valid values come from the snapshot; do not construct them manually.

Jobs reuse authentication, CSRF, audit, idempotency, per-machine locks and durable queue leases. A write freshly reads the BMC and compares the revision before dispatch. SUM receives a minimal XML containing only modified menu paths. Strings, passwords and ordered boot lists are excluded; enum and numeric fields are validated against the exported schema. Conditional settings expose their BIOS help and `WorkIf`; include the corresponding control switch when needed. Vendor numeric bounds are not a recommendation to run hardware at those limits.

The controller adapter (`infrastructure::bios::SumBios` and `runtime/bios-sum.py`) keeps snapshots and operation journals under `/data/bios/<machine UUID>/`. Each operation retains `before.xml`; writes additionally retain `changes.xml` and `rollback.xml`. `/data` is persistent and included in `scripts/backup.sh`. Protect these backups: the full vendor XML may include sensitive configuration. Restore SUM separately from its official distribution. No database migration is needed.

Repeated vendor menu paths (including serial console submenus and some boot entries) are excluded because SUM cannot uniquely address them by name. The rest of the configuration remains available. A regression fixture verifies that even uniquely named children of ambiguous menus cannot be written.

Writes never add SUM's `--reboot` flag. “Succeeded” means SUM accepted the request, not that the running OS adopted it. Pending values remain visible until a later explicit read matches them. External BIOS edits and pre-existing firmware pending changes cannot be reliably detected from SUM's current-config export alone. Complete or reconcile those before using this page.

After dispatch, a timeout or interruption is `unknown`, never an automatic retry. The machine remains locked by the job subsystem until reconciliation/manual resolution. Operation IDs prevent duplicate dispatch; reads or writes take a filesystem lock as a second guard. A cancelled queued job does not run; once SUM has started the operation is allowed to finish and its actual result is retained. No page action restarts miners, restarts the machine, or activates licensing.

For rollback, the original values are in the operation's `rollback.xml`; apply only that machine's file with SUM after checking current state. This release exposes editing, pending state and job history; firmware flashing, arbitrary XML uploads and automatic reboot are excluded.

Validation: `python3 tests/test_bios.py`, workspace Rust checks, generated OpenAPI/TypeScript build, and `frontend/tests/bios.spec.ts` (mocked BIOS write workflow, never a production write).

## Production verification — 2026-09-25

Deployed to the existing controller. Backup: `/home/avalon/rigdeck/backups/bios-page-20260924-235417`; previous image: `rigdeck-app:before-bios-page`. The compose configuration now includes `deploy/compose.bios.yml`; SUM is mounted from `tools/sum`.

All eight `bios_read` API jobs succeeded: node01 has 122 editable settings, node02 has 116, nodes03–08 have 112 each. All report SFT-OOB-LIC. The temporary verification token was revoked. The existing accepted node02 L3 NUMA change was imported into pending state, without issuing another write. The signed-in public browser visibly shows the BIOS navigation, actual node02 configuration and pending Disabled value.

Nine Python contract tests passed on Windows; the first eight also passed inside the Linux release container before the duplicate-path regression was added. Rust clippy passed; the Linux workspace run passed 20 ordinary tests (seven external fixture tests remained ignored). Two browser tests passed against both development assets and the built release assets. Windows execution of one Rust test binary was denied by the host, so the workspace run was completed in Linux. No new production BIOS write or reboot was performed for this release.
