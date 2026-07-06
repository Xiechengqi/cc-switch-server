import { invokeCommand } from "@/lib/runtime";

export async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return invokeCommand<T>(command, args);
}
