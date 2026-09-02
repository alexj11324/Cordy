import { DingTalkMark } from "./dingtalk-mark";
import { LarkMark } from "./lark-mark";
import { LinearMark } from "./linear-mark";
import { SlackMark } from "./slack-mark";
import { TelegramMark } from "./telegram-mark";
import { WecomMark } from "./wecom-mark";
import { WeixinMark } from "./weixin-mark";
import { cn } from "@patchbay/ui/lib/utils";

export type IntegrationChannel = "lark" | "slack" | "dingtalk" | "wecom" | "weixin" | "telegram" | "linear";

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
  const markClassName = size === "lg" ? "h-7 w-7" : "h-4 w-4";
  const icon = {
    lark: <LarkMark className={markClassName} />,
    linear: <LinearMark className={markClassName} />,
    slack: <SlackMark className={markClassName} />,
    dingtalk: <DingTalkMark className={markClassName} />,
    wecom: <WecomMark className={markClassName} />,
    weixin: <WeixinMark className={markClassName} />,
    telegram: <TelegramMark className={markClassName} />,
  }[channel];

  return (
    <span
      aria-hidden="true"
      data-testid={`integration-channel-icon-${channel}`}
      className={cn(
        "flex shrink-0 items-center justify-center text-muted-foreground",
        size === "lg" ? "size-12 rounded-xl bg-surface-muted" : "size-5",
        className,
      )}
    >
      {icon}
    </span>
  );
}
