import { useCallback, useEffect, useState } from 'react';
import { readModelCatalog } from 'cafe-web-sdk';
import type { ModelCatalog } from 'cafe-web-sdk';

const EMPTY: ModelCatalog = { models: [], default_backend: null, backends: [] };

/**
 * Loads the LLM model catalog by reading the `_cafe_llm_registry` session
 * history directly (see `readModelCatalog`). Refreshed on demand.
 */
export function useModels() {
  const [catalog, setCatalog] = useState<ModelCatalog>(EMPTY);

  const refresh = useCallback(async () => {
    try {
      setCatalog(await readModelCatalog());
    } catch (err) {
      console.error('[useModels] failed to read registry session', err);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { catalog, refresh };
}
