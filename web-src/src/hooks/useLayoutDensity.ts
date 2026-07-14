import { useLayoutDensityContext } from "@/components/LayoutDensityProvider";

export function useLayoutDensity() {
  return useLayoutDensityContext();
}
