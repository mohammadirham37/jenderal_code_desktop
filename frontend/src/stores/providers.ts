/** Provider & model catalog state. */
import { create } from "zustand";
import {
  api,
  type ModelInfo,
  type ProviderConfig,
  type ProviderTestResult,
} from "@/lib/ipc";

export interface ProviderPreset {
  id: string;
  label: string;
  kind: ProviderConfig["kind"];
  base_url: string;
  needs_key: boolean;
}

interface ProvidersState {
  providers: ProviderConfig[];
  models: ModelInfo[];
  presets: ProviderPreset[];
  testing: Record<string, boolean>;
  testResults: Record<string, ProviderTestResult>;
  loading: boolean;

  load: () => Promise<void>;
  save: (provider: ProviderConfig, apiKey?: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  test: (id: string) => Promise<void>;
  refreshModels: (id: string) => Promise<void>;
  addCustomModel: (
    providerId: string,
    name: string,
    displayName?: string,
    inputPrice?: number,
    outputPrice?: number,
  ) => Promise<void>;
  deleteModel: (id: string) => Promise<void>;
  modelsOf: (providerId: string) => ModelInfo[];
}

export const useProviders = create<ProvidersState>((set, get) => ({
  providers: [],
  models: [],
  presets: [],
  testing: {},
  testResults: {},
  loading: false,

  load: async () => {
    set({ loading: true });
    try {
      const [providers, models, presets] = await Promise.all([
        api.listProviders(),
        api.listModels(),
        api.providerPresets(),
      ]);
      set({ providers, models, presets });
    } finally {
      set({ loading: false });
    }
  },

  save: async (provider, apiKey) => {
    await api.saveProvider(provider, apiKey);
    set({ providers: await api.listProviders() });
  },

  remove: async (id) => {
    await api.deleteProvider(id);
    set({
      providers: await api.listProviders(),
      models: await api.listModels(),
    });
  },

  test: async (id) => {
    set((s) => ({ testing: { ...s.testing, [id]: true } }));
    try {
      const result = await api.testProvider(id);
      set((s) => ({
        testResults: { ...s.testResults, [id]: result },
      }));
      if (result.ok) {
        await get().refreshModels(id);
      }
    } catch (e) {
      set((s) => ({
        testResults: {
          ...s.testResults,
          [id]: { ok: false, latency_ms: 0, error: String(e) },
        },
      }));
    } finally {
      set((s) => {
        const testing = { ...s.testing };
        delete testing[id];
        return { testing };
      });
    }
  },

  refreshModels: async (id) => {
    await api.refreshModels(id);
    set({ models: await api.listModels() });
  },

  addCustomModel: async (providerId, name, displayName, inputPrice, outputPrice) => {
    await api.addCustomModel(providerId, name, displayName, inputPrice, outputPrice);
    set({ models: await api.listModels() });
  },

  deleteModel: async (id) => {
    await api.deleteModel(id);
    set({ models: await api.listModels() });
  },

  modelsOf: (providerId) => get().models.filter((m) => m.provider_id === providerId),
}));
