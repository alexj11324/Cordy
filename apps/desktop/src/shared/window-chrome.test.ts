// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  NATIVE_WINDOW_CHROME,
  TITLE_BAR_STYLE,
  TRAFFIC_LIGHT_BUTTON,
  TRAFFIC_LIGHT_BUTTON_GAP,
  TRAFFIC_LIGHT_CLUSTER_END,
  TRAFFIC_LIGHT_CONTENT_GAP,
  TRAFFIC_LIGHT_CONTENT_INSET,
  TRAFFIC_LIGHT_POSITION,
  contentInsetAfterTrafficLights,
} from "./window-chrome";

describe("window chrome", () => {
  it("keeps the content inset a single token past the green light", () => {
    expect(TRAFFIC_LIGHT_POSITION).toEqual({ x: 16, y: 17 });
    expect(TRAFFIC_LIGHT_CLUSTER_END).toBe(
      TRAFFIC_LIGHT_POSITION.x +
        3 * TRAFFIC_LIGHT_BUTTON +
        2 * TRAFFIC_LIGHT_BUTTON_GAP,
    );
    expect(TRAFFIC_LIGHT_CONTENT_INSET).toBe(
      TRAFFIC_LIGHT_CLUSTER_END + TRAFFIC_LIGHT_CONTENT_GAP,
    );
    expect(contentInsetAfterTrafficLights(TRAFFIC_LIGHT_CLUSTER_END)).toBe(
      TRAFFIC_LIGHT_CONTENT_INSET,
    );
    expect(contentInsetAfterTrafficLights(0)).toBe(TRAFFIC_LIGHT_CONTENT_GAP);
  });

  it("uses hidden chrome so the titlebar does not swallow pin clicks", () => {
    expect(TITLE_BAR_STYLE).toBe("hidden");
    expect(NATIVE_WINDOW_CHROME).toEqual({
      titleBarStyle: "hidden",
      trafficLightPosition: TRAFFIC_LIGHT_POSITION,
    });
  });
});
