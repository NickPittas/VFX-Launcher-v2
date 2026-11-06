# Back Up Projects

[All guides](../README.md) · [Downloads](../../README.md#downloads)

1. Choose the backup source and destination and review exclusions and Copy workers.
2. Scan folders and review the listed project files and planned destination paths. The backup workflow targets missing/newer `.nk` and `.aep` files and preserves older destination versions under numbered names.
3. Validate the current selection and inspect blocked-item details.
4. Click Run backup. Review progress and final per-file results, including skips and any retained recovery paths.

Use Import job… and Export job… to reuse a backup job configuration. Check imported roots and scheduling settings before running it on another machine.

Configure the schedule controls for the intended cadence and inspect the displayed schedule description. Scheduled execution requires the application to remain open; this is not an installed system backup service. Use Retry only after reading the schedule error and correcting its cause.

A disabled Run backup action displays its blocking reason. Revalidate after changes; older reports and pre-run advisories are historical information. Keep an independent backup until the destination contents and recovery process meet your needs. Network-storage and power-loss behavior are not established by the published local checks.
