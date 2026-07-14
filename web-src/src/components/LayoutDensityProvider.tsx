import * as React from "react";

import {
  applyLayoutDensityClass,
  detectLayoutDensity,
  type LayoutDensity,
} from "@/lib/layout-density";

type LayoutDensityContextValue = {
  density: LayoutDensity;
  isCompact: boolean;
};

const LayoutDensityContext = React.createContext<LayoutDensityContextValue>({
  density: "comfortable",
  isCompact: false,
});

export function useLayoutDensityContext(): LayoutDensityContextValue {
  return React.useContext(LayoutDensityContext);
}

export function LayoutDensityProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [density, setDensity] = React.useState<LayoutDensity>(() =>
    detectLayoutDensity(),
  );

  React.useLayoutEffect(() => {
    const next = detectLayoutDensity();
    setDensity(next);
    applyLayoutDensityClass(next);
    if (typeof document !== "undefined") {
      document.body.dataset.density = next;
    }
  }, []);

  React.useEffect(() => {
    applyLayoutDensityClass(density);
    if (typeof document !== "undefined") {
      document.body.dataset.density = density;
    }
  }, [density]);

  const value = React.useMemo(
    () => ({
      density,
      isCompact: density === "compact",
    }),
    [density],
  );

  return (
    <LayoutDensityContext.Provider value={value}>
      <div className="density-app-surface min-h-screen">{children}</div>
    </LayoutDensityContext.Provider>
  );
}
