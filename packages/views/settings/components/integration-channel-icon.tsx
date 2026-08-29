import { DingTalkMark } from "./dingtalk-mark";
import { LarkMark } from "./lark-mark";
import { SlackMark } from "./slack-mark";
import { TelegramMark } from "./telegram-mark";
import { WecomMark } from "./wecom-mark";
import { WeixinMark } from "./weixin-mark";
import { cn } from "@patchbay/ui/lib/utils";

export type IntegrationChannel =
  | "lark"
  | "slack"
  | "dingtalk"
  | "wecom"
  | "telegram"
  | "weixin";

// Every channel gets its own brand mark, never a generic lucide glyph: the icon
// is what tells a reader which platform the section belongs to, and a stand-in
// speech bubble or plug says nothing (see WecomMark, #6585). lucide-react ships
// no brand icons, so a new channel needs its own `*-mark.tsx` before it can be
// listed here.
export function IntegrationChannelIcon({
  channel,
  className,
  size = "sm",
}: {
  channel: IntegrationChannel;
  className?: string;
  size?: "sm" | "lg";
}) {
  const iconSize = size === "lg" ? "h-7 w-7" : "h-4 w-4";
  const icon = {
    lark: <LarkMark className={iconSize} />,
    slack: <SlackMark className={iconSize} />,
    dingtalk: <DingTalkMark className={iconSize} />,
    wecom: <WecomMark className={iconSize} />,
    telegram: <TelegramMark className={iconSize} />,
    weixin: <WeixinMark className={iconSize} />,
  }[channel];
  const brandColor = {
    lark: "text-[#3370FF]",
    // SlackMark renders its four brand colors internally.
    slack: "text-[#611f69]",
    dingtalk: "text-[#1677FF]",
    wecom: "text-[#07C160]",
    telegram: "text-[#2AABEE]",
    weixin: "text-[#07C160]",
  }[channel];

  return (
    <span
      aria-hidden="true"
      data-testid={`integration-channel-icon-${channel}`}
      className={cn(
        "flex shrink-0 items-center justify-center",
        size === "lg" ? "size-12 rounded-2xl" : "size-5",
        brandColor,
        className,
      )}
    >
      {icon}
    </span>
  );
}
