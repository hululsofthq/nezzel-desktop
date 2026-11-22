import { computed, nextTick, ref } from 'vue';
import { describe, expect, it } from 'vitest';
import { focusSiblingRow } from '../../src/helpers/domIds';
import { useIncrementalRows } from '../../src/composables/useIncrementalRows';

const titles = Array.from({ length: 1000 }, (_, i) => `Clip ${i}`);

function setup() {
  const query = ref('');
  const matches = computed(() => titles.filter(title => title.includes(query.value)));
  return { query, matches, list: useIncrementalRows(matches, query) };
}

describe('useIncrementalRows', () => {
  it('renders the first 200 of 1000, then 400 after Show more, and counts what is left', () => {
    const { list } = setup();
    expect(list.rendered.value).toHaveLength(200);
    expect(list.remaining.value).toBe(800);
    list.showMore();
    expect(list.rendered.value).toHaveLength(400);
    expect(list.remaining.value).toBe(600);
    expect(list.rendered.value[399]).toBe('Clip 399');
  });

  it('offers only what is left on the last page', () => {
    const { list } = setup();
    for (let i = 0; i < 4; i++) list.showMore();
    expect(list.rendered.value).toHaveLength(1000);
    expect(list.nextCount.value).toBe(0);
    const short = useIncrementalRows(ref(titles.slice(0, 250)), ref(0));
    short.showMore();
    expect(short.remaining.value).toBe(0);
  });

  it('search matches a row beyond the first 200 and goes back to one page when the query changes', async () => {
    const { query, matches, list } = setup();
    list.showMore();
    query.value = 'Clip 9';
    await nextTick();
    expect(matches.value).toContain('Clip 950');
    expect(list.rendered.value).toHaveLength(111);
    query.value = '';
    await nextTick();
    expect(list.rendered.value).toHaveLength(200);
    query.value = 'Clip 5';
    await nextTick();
    expect(list.rendered.value).toHaveLength(111);
    expect(list.rendered.value).toContain('Clip 599');
  });

  it('reveals a row beyond the rendered page', () => {
    const { list } = setup();
    list.reveal(650);
    expect(list.rendered.value).toHaveLength(651);
  });

  it('ArrowDown on the last rendered row shows the next page and focuses the new row', async () => {
    const { list } = setup();
    const host = document.createElement('div');
    document.body.append(host);
    const paint = () => {
      host.innerHTML = list.rendered.value.map(() => '<li class="row" tabindex="0"></li>').join('') + '<button data-show-more></button>';
      host.querySelector('button')!.onclick = () => {
        list.showMore();
        paint();
      };
    };
    paint();
    const rows = () => Array.from(host.querySelectorAll<HTMLElement>('li.row'));
    rows()[198].focus();
    focusSiblingRow(rows()[198], 1);
    expect(document.activeElement).toBe(rows()[199]);
    focusSiblingRow(rows()[199], 1);
    await nextTick();
    expect(rows()).toHaveLength(400);
    expect(document.activeElement).toBe(rows()[200]);
    focusSiblingRow(rows()[200], -1);
    expect(document.activeElement).toBe(rows()[199]);
    host.remove();
  });

  it('ArrowDown on the very last row, with nothing left to show, stays put', async () => {
    document.body.innerHTML = '<li class="row" tabindex="0"></li>';
    const only = document.querySelector<HTMLElement>('li.row')!;
    only.focus();
    focusSiblingRow(only, 1);
    await nextTick();
    expect(document.activeElement).toBe(only);
    document.body.innerHTML = '';
  });
});
