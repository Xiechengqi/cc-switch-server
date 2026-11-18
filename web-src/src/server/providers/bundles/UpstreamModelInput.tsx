import { BuddyUpstreamModelInput, type BuddyModelCatalogControl } from "./BuddyModelSelector";
import { ZenUpstreamModelInput, type ZenModelCatalogControl } from "./ZenModelSelector";
import type { ModelInputProps } from "./EditableModelInput";

export function UpstreamModelInput({ catalog, zenCatalog, ...input }: ModelInputProps & {
  catalog?: BuddyModelCatalogControl;
  zenCatalog?: ZenModelCatalogControl;
}) {
  return zenCatalog?.supported
    ? <ZenUpstreamModelInput {...input} catalog={zenCatalog} />
    : <BuddyUpstreamModelInput {...input} catalog={catalog} />;
}
