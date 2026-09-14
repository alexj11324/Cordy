const {
  IOSConfig,
  withAppDelegate,
  withEntitlementsPlist,
  withInfoPlist,
} = require("expo/config-plugins");

const sceneDelegate = `import UIKit
import React

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
  var window: UIWindow?

  func scene(
    _ scene: UIScene,
    willConnectTo session: UISceneSession,
    options connectionOptions: UIScene.ConnectionOptions
  ) {
    guard
      let windowScene = scene as? UIWindowScene,
      let appDelegate = UIApplication.shared.delegate as? AppDelegate,
      let factory = appDelegate.reactNativeFactory
    else {
      return
    }

    let window = UIWindow(windowScene: windowScene)
    self.window = window
    factory.startReactNative(
      withModuleName: "main",
      in: window,
      launchOptions: nil
    )
    window.makeKeyAndVisible()

    for context in connectionOptions.urlContexts {
      _ = RCTLinkingManager.application(
        UIApplication.shared,
        open: context.url,
        options: [:]
      )
    }
  }

  func scene(
    _ scene: UIScene,
    openURLContexts URLContexts: Set<UIOpenURLContext>
  ) {
    for context in URLContexts {
      _ = RCTLinkingManager.application(
        UIApplication.shared,
        open: context.url,
        options: [:]
      )
    }
  }

  func scene(
    _ scene: UIScene,
    continue userActivity: NSUserActivity
  ) {
    _ = RCTLinkingManager.application(
      UIApplication.shared,
      continue: userActivity,
      restorationHandler: { _ in }
    )
  }
}
`;

const sceneLifecycleComment =
  "// The scene delegate owns the window and starts React Native for iOS 27+.";
const expoWindowStartup = `#if os(iOS) || os(tvOS)
    window = UIWindow(frame: UIScreen.main.bounds)
    factory.startReactNative(
      withModuleName: "main",
      in: window,
      launchOptions: launchOptions)
#endif`;

function removeExpoWindowStartup(contents) {
  if (contents.includes(sceneLifecycleComment)) return contents;
  if (!contents.includes(expoWindowStartup)) {
    throw new Error(
      "Expected Expo AppDelegate startup block was not found while enabling UIScene lifecycle.",
    );
  }
  return contents.replace(expoWindowStartup, sceneLifecycleComment);
}

function removeAppleSignInEntitlement(entitlements) {
  const next = { ...entitlements };
  delete next["com.apple.developer.applesignin"];
  return next;
}

function withIOSSceneLifecycle(config) {
  config = withInfoPlist(config, (config) => {
    config.modResults.UIApplicationSceneManifest = {
      UIApplicationSupportsMultipleScenes: false,
      UISceneConfigurations: {
        UIWindowSceneSessionRoleApplication: [
          {
            UISceneConfigurationName: "Default Configuration",
            UISceneDelegateClassName: "$(PRODUCT_MODULE_NAME).SceneDelegate",
          },
        ],
      },
    };
    return config;
  });

  // @clerk/expo is configured with appleSignIn: false. Remove the key if an
  // older incremental prebuild left it behind in the generated project.
  config = withEntitlementsPlist(config, (config) => {
    config.modResults = removeAppleSignInEntitlement(config.modResults);
    return config;
  });

  config = withAppDelegate(config, (config) => {
    config.modResults.contents = removeExpoWindowStartup(
      config.modResults.contents,
    );
    return config;
  });

  return IOSConfig.XcodeProjectFile.withBuildSourceFile(config, {
    filePath: "SceneDelegate.swift",
    contents: sceneDelegate,
    overwrite: true,
  });
}

module.exports = withIOSSceneLifecycle;
module.exports.removeAppleSignInEntitlement = removeAppleSignInEntitlement;
module.exports.removeExpoWindowStartup = removeExpoWindowStartup;
