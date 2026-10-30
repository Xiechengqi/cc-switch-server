import {
  providersApi,
  type ProviderBundleRebindPreview,
  type ProviderBundleView,
  type ProviderBundleWriteDraft,
} from "@/lib/api/providers";
import { HttpResponseError } from "@/lib/runtime";
import { stableStringify } from "@/lib/stableStringify";
import {
  editProviderBundleDraft,
  toProviderBundleWriteDraft,
  type ProviderBundleEditorDraft,
} from "./bundleDraft";

export interface BundleRebindConfirmation {
  writeDraft: ProviderBundleWriteDraft;
  preview: ProviderBundleRebindPreview;
}

export class BundleSaveUncertainError extends Error {
  constructor(public readonly latest?: ProviderBundleView) {
    super(
      "Provider save result is uncertain; review the latest configuration before retrying",
    );
    this.name = "BundleSaveUncertainError";
  }
}

/**
 * Rebase only when read-back matches the requested non-secret configuration.
 * Creation keeps its original idempotency key; unrelated concurrent edits must
 * still fail the original revision check instead of being overwritten. A newer
 * revision cannot prove that a redacted credential replacement was committed.
 */
export function reconcileBundleSaveDraft(
  draft: ProviderBundleEditorDraft,
  latest: ProviderBundleView,
): ProviderBundleEditorDraft | undefined {
  if (
    draft.expectedRevision === undefined ||
    latest.id !== draft.id ||
    latest.revision < draft.expectedRevision ||
    latest.revision > draft.expectedRevision + 1
  ) {
    return;
  }
  const requested = toProviderBundleWriteDraft(draft);
  const credentialPatches = [
    requested.credentialPatches,
    ...requested.surfaces.map((surface) => surface.credentialPatches),
  ];
  if (
    latest.revision !== draft.expectedRevision &&
    credentialPatches.some((patches) =>
      Object.values(patches ?? {}).some((patch) => patch.action !== "keep"),
    )
  ) {
    return;
  }
  const projection = (write: ProviderBundleWriteDraft) => {
    const {
      expectedRevision: _revision,
      clientRequestId: _request,
      credentialPatches: _credentials,
      ...config
    } = write;
    return {
      ...config,
      surfaces: config.surfaces.map(
        ({ credentialPatches: _patches, ...surface }) => surface,
      ),
    };
  };
  const observed = toProviderBundleWriteDraft(editProviderBundleDraft(latest));
  if (
    stableStringify(projection(requested)) !==
    stableStringify(projection(observed))
  ) {
    return;
  }
  return {
    ...draft,
    expectedRevision: latest.revision,
    clientRequestId: undefined,
  };
}

// A lost response is not proof of failure. Read back, preserve the user's draft,
// and require another explicit save instead of repeating credential changes.
async function writeProvider(
  id: string,
  write: () => Promise<ProviderBundleView>,
): Promise<ProviderBundleView> {
  try {
    return await write();
  } catch (error) {
    if (
      error instanceof HttpResponseError &&
      error.status < 500 &&
      error.status !== 408
    ) {
      throw error;
    }
    const latest = await providersApi.getBundle(id).catch(() => undefined);
    throw new BundleSaveUncertainError(latest);
  }
}

export async function prepareBundleProviderSave(
  draft: ProviderBundleEditorDraft,
  baseline?: ProviderBundleView,
): Promise<
  | { kind: "saved"; bundle: ProviderBundleView }
  | { kind: "confirm"; confirmation: BundleRebindConfirmation }
> {
  if (
    baseline &&
    stableStringify(draft) ===
      stableStringify(editProviderBundleDraft(baseline))
  ) {
    return { kind: "saved", bundle: baseline };
  }
  const writeDraft = structuredClone(toProviderBundleWriteDraft(draft));
  const changesBinding =
    baseline &&
    writeDraft.surfaces.some((surface) => {
      const before = baseline.surfaces[surface.app]?.customBinding;
      return (
        before &&
        stableStringify(before) !== stableStringify(surface.customBinding)
      );
    });
  if (changesBinding) {
    const preview = await providersApi.previewBundleRebind(writeDraft);
    return { kind: "confirm", confirmation: { writeDraft, preview } };
  }
  return {
    kind: "saved",
    bundle: await writeProvider(draft.id, () =>
      providersApi.upsertBundle(writeDraft),
    ),
  };
}

export async function applyBundleProviderSave(
  draft: ProviderBundleEditorDraft,
  confirmation: BundleRebindConfirmation,
): Promise<ProviderBundleView> {
  if (
    stableStringify(toProviderBundleWriteDraft(draft)) !==
    stableStringify(confirmation.writeDraft)
  ) {
    throw new HttpResponseError(
      "Provider draft changed; preview the binding changes again",
      409,
      { code: "cc_switch_provider_preview_conflict" },
    );
  }
  return writeProvider(draft.id, () =>
    providersApi.applyBundleRebind(
      confirmation.writeDraft,
      confirmation.preview.previewToken,
    ),
  );
}
