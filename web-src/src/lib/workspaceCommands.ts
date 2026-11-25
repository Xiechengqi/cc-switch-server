import { createContext, useContext } from "react";

export const WorkspaceCommands = createContext<ReadonlySet<string> | null>(null);

export function useWorkspaceCommand(command: string): boolean {
  const commands = useContext(WorkspaceCommands);
  return commands === null || commands.has(command);
}
