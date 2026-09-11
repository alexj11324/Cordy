/**
 * Native macOS window-control geometry.
 *
 * Traffic lights are not in the DOM, and Electron only exposes
 * `env(titlebar-area-*)` for Window Controls Overlay — which this shell
 * does not use. The one number the renderer is allowed to know is the
 * cluster's right edge, derived from the same origin the BrowserWindow
 * is constructed with. Collapsed chrome then contains that edge in the
 * icon rail so the pane header can stay ordinary flex layout.
 *
 * `hiddenInset` keeps a native titlebar hit target that swallows clicks in
 * the first ~38px (pin, breadcrumb, header actions). `hidden` plus the same
 * `trafficLightPosition` is the documented equivalent without that dead zone.
 */

export const TITLE_BAR_STYLE = "hidden" as const;

export const TRAFFIC_LIGHT_POSITION = { x: 16, y: 17 } as const;

export const NATIVE_WINDOW_CHROME = {
  titleBarStyle: TITLE_BAR_STYLE,
  trafficLightPosition: TRAFFIC_LIGHT_POSITION,
} as const;

/** Apple HIG diameter of each traffic-light button. */
export const TRAFFIC_LIGHT_BUTTON = 12;
/** Apple HIG gap between the three buttons. */
export const TRAFFIC_LIGHT_BUTTON_GAP = 8;
const TRAFFIC_LIGHT_COUNT = 3;

/** Right edge of the green light, in window coordinates. */
export const TRAFFIC_LIGHT_CLUSTER_END =
  TRAFFIC_LIGHT_POSITION.x +
  TRAFFIC_LIGHT_COUNT * TRAFFIC_LIGHT_BUTTON +
  (TRAFFIC_LIGHT_COUNT - 1) * TRAFFIC_LIGHT_BUTTON_GAP;

/** Same token as the inset gutter (`spacing-2`). */
export const TRAFFIC_LIGHT_CONTENT_GAP = 8;

/**
 * Where web chrome may start when it shares the titlebar with the lights.
 * Always `clusterEnd + gutter` so macOS and other hosts share one formula.
 */
export const TRAFFIC_LIGHT_CONTENT_INSET =
  TRAFFIC_LIGHT_CLUSTER_END + TRAFFIC_LIGHT_CONTENT_GAP;

/** Leading inset for a pin/control after a given traffic-light cluster. */
export function contentInsetAfterTrafficLights(clusterEndPx: number): number {
  return clusterEndPx + TRAFFIC_LIGHT_CONTENT_GAP;
}

/** Right edge of the traffic-light cluster for the current window mode. */
export function trafficLightClusterEndForWindow(isFullScreen: boolean): number {
  return isFullScreen ? 0 : TRAFFIC_LIGHT_CLUSTER_END;
}
