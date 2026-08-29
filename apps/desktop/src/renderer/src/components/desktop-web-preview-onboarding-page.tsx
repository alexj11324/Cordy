import {
  CliInstallInstructions,
  OnboardingFlow,
} from "@patchbay/views/onboarding";

export function DesktopWebPreviewOnboardingPage() {
  return (
    <div
      className="h-full overflow-y-auto bg-background"
      data-preview-no-backend="true"
    >
      <OnboardingFlow
        singlePane
        onComplete={() => undefined}
        runtimeInstructions={<CliInstallInstructions />}
      />
    </div>
  );
}
