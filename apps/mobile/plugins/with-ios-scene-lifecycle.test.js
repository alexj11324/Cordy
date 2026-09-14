const assert = require("node:assert/strict");
const test = require("node:test");

const {
  removeAppleSignInEntitlement,
  removeExpoWindowStartup,
} = require("./with-ios-scene-lifecycle");

const STARTUP_BLOCK = `#if os(iOS) || os(tvOS)
    window = UIWindow(frame: UIScreen.main.bounds)
    factory.startReactNative(
      withModuleName: "main",
      in: window,
      launchOptions: launchOptions)
#endif`;

test("scene lifecycle transform is idempotent across repeated prebuilds", () => {
  const firstPass = removeExpoWindowStartup(`prefix\n${STARTUP_BLOCK}\nsuffix`);

  assert.match(firstPass, /scene delegate owns the window/u);
  assert.equal(removeExpoWindowStartup(firstPass), firstPass);
});

test("stale Apple sign-in entitlement is removed when disabled in app config", () => {
  const entitlements = {
    "com.apple.developer.applesignin": ["Default"],
    "aps-environment": "development",
  };

  assert.deepEqual(removeAppleSignInEntitlement(entitlements), {
    "aps-environment": "development",
  });
});
