"use client";

import { Cpu, MessageSquare, Sparkles, Wrench } from "lucide-react";
import {
  Frame,
  FrameDescription,
  FrameHeader,
  FrameTitle,
} from "@orvilo/ui/components/reui/frame";
import { IconTile } from "@orvilo/ui/components/reui/icon-tile";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { paths, useWorkspaceSlug } from "@orvilo/core/paths";
import { useNavigation } from "../../navigation";

interface RelatedControlItemData {
  title: string;
  description: string;
  badge: string;
  icon: React.ReactNode;
  href: string;
}

export function RelatedControls({ agentId: _agentId }: { agentId: string }) {
  const slug = useWorkspaceSlug();
  const wsPaths = slug ? paths.workspace(slug) : null;
  const navigation = useNavigation();

  const controls: RelatedControlItemData[] = [
    {
      title: "技能配置 (Skills)",
      description: "管理为该智能体开放的代码分析、测试、检索与系统指令扩展技能包。",
      badge: "扩展能力",
      icon: <Sparkles className="size-4.5 text-amber-500" />,
      href: wsPaths ? wsPaths.skills() : "/skills",
    },
    {
      title: "MCP 工具协议 (MCP Servers)",
      description: "连接并调用工作区绑定的 Model Context Protocol 外部数据与工具服务。",
      badge: "协议集成",
      icon: <Wrench className="size-4.5 text-blue-500" />,
      href: wsPaths ? wsPaths.skills() : "/skills",
    },
    {
      title: "运行宿主 (Runtimes)",
      description: "查看智能体分配的本地或云端执行环境、并发容量与健康状态。",
      badge: "基础设施",
      icon: <Cpu className="size-4.5 text-emerald-500" />,
      href: wsPaths ? wsPaths.runtimes() : "/runtimes",
    },
    {
      title: "对话与联调 (Chat & Debug)",
      description: "在交互式对话框中对该智能体发起即时任务，查看思维链与步骤流式输出。",
      badge: "交互测试",
      icon: <MessageSquare className="size-4.5 text-purple-500" />,
      href: wsPaths ? wsPaths.chat() : "/chat",
    },
  ];

  return (
    <Frame className="w-full">
      <FrameHeader className="px-1 py-1">
        <FrameTitle>关联系统与控制台 (Related Controls)</FrameTitle>
        <FrameDescription className="flex items-center gap-2">
          <span>4 项快速联调入口</span>
          <span
            aria-hidden="true"
            className="size-1 rounded-full bg-muted-foreground/50"
          />
          <span>工作区统一治理</span>
        </FrameDescription>
      </FrameHeader>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {controls.map((item) => (
          <button
            key={item.title}
            type="button"
            onClick={() => navigation.push(item.href)}
            className="group/card relative flex flex-col justify-between overflow-hidden rounded-xl border border-border/70 bg-card/60 p-4.5 text-left shadow-2xs transition-all duration-200 hover:-translate-y-0.5 hover:border-primary/40 hover:bg-card hover:shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <div>
              <div className="flex items-center justify-between gap-2 mb-3">
                <IconTile variant="elevated" size="default" className="transition-transform group-hover/card:scale-105">
                  {item.icon}
                </IconTile>
                <Badge variant="outline" size="xs">
                  {item.badge}
                </Badge>
              </div>
              <h4 className="text-body font-semibold text-foreground group-hover/card:text-primary transition-colors">
                {item.title}
              </h4>
              <p className="mt-1.5 text-caption text-muted-foreground leading-relaxed">
                {item.description}
              </p>
            </div>
          </button>
        ))}
      </div>
    </Frame>
  );
}
