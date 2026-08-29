"use client";

import {
  CliInstallInstructions,
  OnboardingFlow,
} from "@patchbay/views/onboarding";
import { PreviewSession } from "../preview-session";

export default function UiPreviewOnboardingPage() {
  return (
    <PreviewSession onboarded={false}>
      <div className="h-full overflow-y-auto bg-background">
        <OnboardingFlow
          singlePane
          backendFree
          onComplete={() => undefined}
          runtimeInstructions={<CliInstallInstructions />}
        />
      </div>
    </PreviewSession>
  );
}
