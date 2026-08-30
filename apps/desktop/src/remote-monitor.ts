/**
 * Live SSH monitoring is deliberately low frequency. Keep the allowed values
 * in one small, renderer-testable module so a future settings surface cannot
 * accidentally create an unbounded polling interval.
 */
export const REMOTE_MONITOR_REFRESH_INTERVALS = [15, 30, 60] as const;

export type RemoteMonitorRefreshInterval = (typeof REMOTE_MONITOR_REFRESH_INTERVALS)[number];

export function isRemoteMonitorRefreshInterval(value: number): value is RemoteMonitorRefreshInterval {
  return REMOTE_MONITOR_REFRESH_INTERVALS.includes(value as RemoteMonitorRefreshInterval);
}
