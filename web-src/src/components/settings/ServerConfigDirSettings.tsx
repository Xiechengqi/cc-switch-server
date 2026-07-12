import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";

interface ServerConfigDirSettingsProps {
  configDir: string;
}

/** Read-only server data directory; token server does not manage per-client config paths. */
export function ServerConfigDirSettings({
  configDir,
}: ServerConfigDirSettingsProps) {
  const { t } = useTranslation();

  return (
    <section className="space-y-4">
      <header className="space-y-1">
        <h3 className="text-sm font-medium">
          {t("settings.serverConfigDir.title", {
            defaultValue: "Server 配置目录",
          })}
        </h3>
        <p className="text-xs text-muted-foreground">
          {t("settings.serverConfigDir.description", {
            defaultValue:
              "providers、shares、ui-settings 等持久化文件所在目录。监听地址与端口由进程启动参数决定。",
          })}
        </p>
      </header>
      <Input value={configDir} readOnly className="font-mono text-sm" />
    </section>
  );
}
