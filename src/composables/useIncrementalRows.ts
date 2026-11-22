import { computed, ref, watch, type Ref, type WatchSource } from 'vue';

/**
 * Renders only the first `page` rows of a long list and grows by `page` on request. Data work (search,
 * counts, selection) stays on the full list; `reset` (filter or search change) goes back to one page.
 */
export function useIncrementalRows<T>(all: Ref<readonly T[]>, reset: WatchSource, page = 200) {
  const limit = ref(page);
  watch(reset, () => {
    limit.value = page;
  });
  const rendered = computed(() => all.value.slice(0, limit.value));
  const remaining = computed(() => Math.max(0, all.value.length - limit.value));
  const nextCount = computed(() => Math.min(page, remaining.value));
  const showMore = () => {
    limit.value += page;
  };
  /** Makes sure the row at `index` of the full list is rendered (e.g. before focusing it). */
  const reveal = (index: number) => {
    if (index >= limit.value) limit.value = index + 1;
  };
  return { rendered, remaining, nextCount, showMore, reveal };
}
