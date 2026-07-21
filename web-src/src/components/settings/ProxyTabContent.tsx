import { Activity, Server } from "lucide-react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { ProxyPanel } from "@/components/proxy";
import { useProxyStatus } from "@/hooks/useProxyStatus";

export function ProxyTabContent() {
  const { t } = useTranslation();
  const { isRunning } = useProxyStatus();

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
    >
      <Accordion type="single" collapsible defaultValue="proxy">
        <AccordionItem
          value="proxy"
          className="overflow-hidden rounded-lg glass-card"
        >
          <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
            <div className="flex min-w-0 flex-1 items-center gap-3">
              <Server className="h-5 w-5 shrink-0 text-green-500" />
              <div className="min-w-0 text-left">
                <h3 className="text-base font-semibold">
                  {t("settings.advanced.proxy.title")}
                </h3>
                <p className="text-sm font-normal text-muted-foreground">
                  {t("settings.advanced.proxy.description")}
                </p>
              </div>
              <Badge
                variant={isRunning ? "default" : "secondary"}
                className="ml-auto mr-2 h-6 shrink-0 gap-1.5"
              >
                <Activity
                  className={`h-3 w-3 ${isRunning ? "animate-pulse" : ""}`}
                />
                {isRunning
                  ? t("settings.advanced.proxy.running")
                  : t("settings.advanced.proxy.stopped")}
              </Badge>
            </div>
          </AccordionTrigger>
          <AccordionContent className="border-t border-border/50 px-6 pb-6 pt-4">
            <ProxyPanel />
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </motion.div>
  );
}
