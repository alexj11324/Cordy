"use client";

import { useState } from "react";
import { Globe, Lock, RotateCcw, ShieldCheck } from "lucide-react";
import { Badge } from "@orvilo/ui/components/reui/badge";
import {
  Frame,
  FrameDescription,
  FrameHeader,
  FramePanel,
  FrameTitle,
} from "@orvilo/ui/components/reui/frame";
import { IconTile } from "@orvilo/ui/components/reui/icon-tile";
import { Switch } from "@orvilo/ui/components/ui/switch";

interface PolicyItem {
  id: string;
  title: string;
  description: string;
  icon: React.ReactNode;
  badge: {
    label: string;
    variant: "info-light" | "success-light" | "warning-light" | "outline";
  };
  defaultEnabled: boolean;
}

const DEFAULT_POLICIES: PolicyItem[] = [
  {
    id: "sandbox",
    title: "沙箱隔离运行 (Sandbox Execution)",
    description: "在受限沙箱中运行智能体代码与命令，隔离主机文件系统与系统敏感路径。",
    icon: <ShieldCheck className="size-4 text-primary" />,
    badge: { label: "安全防护", variant: "info-light" },
    defaultEnabled: true,
  },
  {
    id: "retry",
    title: "故障自动重试 (Auto-Retry Policy)",
    description: "当调用底层大模型遇到网络波动或临时限流错误时，按指数退避自动重试最多 3 次。",
    icon: <RotateCcw className="size-4 text-amber-500" />,
    badge: { label: "高可用", variant: "warning-light" },
    defaultEnabled: true,
  },
  {
    id: "network",
    title: "公网外部访问 (Outbound Network Access)",
    description: "允许智能体在执行工具时向外部互联网发送 HTTP/HTTPS 请求与抓取网页内容。",
    icon: <Globe className="size-4 text-emerald-500" />,
    badge: { label: "网络策略", variant: "success-light" },
    defaultEnabled: true,
  },
];

export function OperatingPolicies({
  canEdit = true,
}: {
  canEdit?: boolean;
}) {
  const [policies, setPolicies] = useState<Record<string, boolean>>({
    sandbox: true,
    retry: true,
    network: true,
  });

  const handleToggle = (id: string, value: boolean) => {
    if (!canEdit) return;
    setPolicies((prev) => ({ ...prev, [id]: value }));
  };

  return (
    <Frame className="w-full">
      <FrameHeader className="px-1 py-1">
        <FrameTitle>运行策略与安全护栏 (Operating Policies)</FrameTitle>
        <FrameDescription className="flex items-center gap-2">
          <span>3 条自定义策略</span>
          <span
            aria-hidden="true"
            className="size-1 rounded-full bg-muted-foreground/50"
          />
          <span>1 项组织强制锁定</span>
        </FrameDescription>
      </FrameHeader>

      <FramePanel className="p-0 divide-y divide-border/60">
        {DEFAULT_POLICIES.map((item) => {
          const isChecked = policies[item.id] ?? item.defaultEnabled;
          return (
            <div
              key={item.id}
              className="flex items-center justify-between gap-4 px-5 py-4 transition-colors hover:bg-muted/20"
            >
              <div className="flex items-center gap-3.5 min-w-0">
                <IconTile variant="elevated" size="default">
                  {item.icon}
                </IconTile>
                <div className="min-w-0 space-y-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-body font-medium text-foreground">
                      {item.title}
                    </span>
                    <Badge variant={item.badge.variant} size="xs">
                      {item.badge.label}
                    </Badge>
                  </div>
                  <p className="text-caption text-muted-foreground leading-relaxed">
                    {item.description}
                  </p>
                </div>
              </div>

              <div className="shrink-0 pl-2">
                <Switch
                  checked={isChecked}
                  onCheckedChange={(checked) => handleToggle(item.id, checked)}
                  disabled={!canEdit}
                  aria-label={item.title}
                />
              </div>
            </div>
          );
        })}

        {/* Organization Locked Policy Row */}
        <div className="flex items-center justify-between gap-4 px-5 py-4 bg-muted/10">
          <div className="flex items-center gap-3.5 min-w-0">
            <IconTile variant="elevated" size="default">
              <Lock className="size-4 text-muted-foreground" />
            </IconTile>
            <div className="min-w-0 space-y-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-body font-medium text-foreground">
                  全生命周期审计录制 (Org Audit & Telemetry)
                </span>
                <Badge variant="outline" size="xs">
                  <span className="size-1.5 rounded-full bg-info mr-1" aria-hidden="true" />
                  组织策略锁定
                </Badge>
              </div>
              <p className="text-caption text-muted-foreground leading-relaxed">
                依据工作区合规配置，智能体的所有提示词输入、工具调用及执行日志均强制录制归档。
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2.5 shrink-0 pl-2">
            <Badge variant="outline" size="xs">
              <span className="size-1.5 rounded-full bg-success mr-1" aria-hidden="true" />
              强制启用
            </Badge>
            <Switch checked disabled aria-label="全生命周期审计录制" />
          </div>
        </div>
      </FramePanel>
    </Frame>
  );
}
