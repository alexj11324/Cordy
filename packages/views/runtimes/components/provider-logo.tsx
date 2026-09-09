import { Monitor } from "lucide-react";
import antigravityLogo from "./provider-icons/antigravity.png";
import claudeLogo from "./provider-icons/claude.svg";
import codebuddyLogo from "./provider-icons/codebuddy.svg";
import codexLogo from "./provider-icons/codex.svg";
import copilotLogo from "./provider-icons/copilot.svg";
import cursorLogo from "./provider-icons/cursor.svg";
import devecoLogo from "./provider-icons/deveco.png";
import dshLogo from "./provider-icons/dsh.svg";
import geminiLogo from "./provider-icons/gemini.svg";
import grokLogo from "./provider-icons/grok.svg";
import hermesLogo from "./provider-icons/hermes.webp";
import kimiLogo from "./provider-icons/kimi.svg";
import kiroLogo from "./provider-icons/kiro.svg";
import mcodeLogo from "./provider-icons/mcode.svg";
import ompLogo from "./provider-icons/omp.svg";
import openclawLogo from "./provider-icons/openclaw.svg";
import opencodeLogo from "./provider-icons/opencode.svg";
import piLogo from "./provider-icons/pi.svg";
import qoderLogo from "./provider-icons/qoder.svg";
import qoderclicnLogo from "./provider-icons/qoderclicn.svg";
import qwenLogo from "./provider-icons/qwen.svg";
import qwenpawLogo from "./provider-icons/qwenpaw.svg";
import reasonixLogo from "./provider-icons/reasonix.svg";
import traecliLogo from "./provider-icons/traecli.png";
import codeartsLogo from "./codearts-logo.svg";
import dimLogo from "./dim-logo.png";

// Next.js exposes static imports as objects while Vite exposes URL strings.
// Normalize both shapes here so shared provider logos work in web and desktop.
function staticAssetSrc(asset: string | { src: string }): string {
  return typeof asset === "string" ? asset : asset.src;
}

const coordyProviderIcons = {
  antigravity: staticAssetSrc(antigravityLogo),
  claude: staticAssetSrc(claudeLogo),
  codebuddy: staticAssetSrc(codebuddyLogo),
  codex: staticAssetSrc(codexLogo),
  copilot: staticAssetSrc(copilotLogo),
  cursor: staticAssetSrc(cursorLogo),
  deveco: staticAssetSrc(devecoLogo),
  dsh: staticAssetSrc(dshLogo),
  gemini: staticAssetSrc(geminiLogo),
  grok: staticAssetSrc(grokLogo),
  hermes: staticAssetSrc(hermesLogo),
  kimi: staticAssetSrc(kimiLogo),
  kiro: staticAssetSrc(kiroLogo),
  mcode: staticAssetSrc(mcodeLogo),
  omp: staticAssetSrc(ompLogo),
  openclaw: staticAssetSrc(openclawLogo),
  opencode: staticAssetSrc(opencodeLogo),
  pi: staticAssetSrc(piLogo),
  qoder: staticAssetSrc(qoderLogo),
  qoderclicn: staticAssetSrc(qoderclicnLogo),
  qwen: staticAssetSrc(qwenLogo),
  qwenpaw: staticAssetSrc(qwenpawLogo),
  reasonix: staticAssetSrc(reasonixLogo),
  traecli: staticAssetSrc(traecliLogo),
} as const;

function ZeroClawLogo({ className }: { className: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      className={className}
    >
      <path d="M5 5l4 14" />
      <path d="M12 5l2 14" />
      <path d="M19 5l-4 14" />
    </svg>
  );
}

export function ProviderLogo({
  provider,
  className = "h-4 w-4",
}: {
  provider: string;
  className?: string;
}) {
  const coordySrc =
    coordyProviderIcons[provider as keyof typeof coordyProviderIcons];
  if (coordySrc) {
    return <img src={coordySrc} alt="" aria-hidden className={className} />;
  }

  if (provider === "codearts") {
    return (
      <img
        src={staticAssetSrc(codeartsLogo)}
        alt="CodeArts"
        className={className}
      />
    );
  }

  if (provider === "dim") {
    return (
      <img
        src={staticAssetSrc(dimLogo)}
        alt=""
        aria-hidden
        className={className}
      />
    );
  }

  if (provider === "zeroclaw") {
    return <ZeroClawLogo className={className} />;
  }

  return <Monitor className={className} />;
}
