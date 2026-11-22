import { describe, expect, it } from 'vitest';
import { groupFolderValues } from '../../src/helpers/groupFolder';

describe('group folder values', () => {
  it('leaves a single file flat', () => {
    expect(groupFolderValues({ files: 1, title: 'Solo' })).toEqual({});
    expect(groupFolderValues({ files: 0 })).toEqual({});
  });

  it('names a group of two or more after its title', () => {
    expect(groupFolderValues({ files: 3, title: '  رحلة  ' })).toEqual({
      nezzel_group_files: '3',
      nezzel_group_title: 'رحلة',
    });
  });

  it('falls back to uploader and id, then to the site', () => {
    expect(groupFolderValues({ files: 2, uploader: 'nasa', id: 'PL1' }).nezzel_group_title).toBe('nasa - PL1');
    expect(groupFolderValues({ files: 2, uploader: 'nasa' }).nezzel_group_title).toBe('nasa');
    expect(groupFolderValues({ files: 2, url: 'https://www.flickr.com/photos/x/albums/1' }).nezzel_group_title).toBe('flickr.com');
  });
});
