import { useCallback, useEffect, useState } from "react";

export function useUnsavedChangesGuard({
  active,
  dirty,
  onClose,
}: {
  active: boolean;
  dirty: boolean;
  onClose: () => void;
}) {
  const [confirmOpen, setConfirmOpen] = useState(false);

  useEffect(() => {
    if (!active || !dirty) return;
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, [active, dirty]);

  useEffect(() => {
    if (!active) setConfirmOpen(false);
  }, [active]);

  const requestClose = useCallback(() => {
    if (dirty) {
      setConfirmOpen(true);
      return;
    }
    onClose();
  }, [dirty, onClose]);

  const discardAndClose = useCallback(() => {
    setConfirmOpen(false);
    onClose();
  }, [onClose]);

  return {
    confirmOpen,
    requestClose,
    discardAndClose,
    keepEditing: () => setConfirmOpen(false),
  };
}
