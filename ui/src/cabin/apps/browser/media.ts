import { create } from 'zustand'
import { postToHost, type BrowserMedia } from '../../../shared/host'
import type { MediaAction, MediaSource } from '../../../shared/media'
import type { BrowserId } from '../../../shared/webApps'

export function browserSource(id: BrowserId, media: BrowserMedia | null | undefined): MediaSource | null {
  if (!media) return null
  return {
    id: `westplay-${id}`,
    service: 'browser',
    title: media.title,
    artist: media.artist,
    album: media.album,
    artwork: media.artwork,
    status: media.playing ? 'playing' : 'paused',
    positionMs: 0,
    positionAt: 0,
    durationMs: 0,
    controls: { playPause: media.playPause, next: media.next, previous: media.previous, shuffle: false, repeat: false },
    shuffle: null,
    repeat: null,
  }
}

export const useBrowserMedia = create<Record<BrowserId, MediaSource | null>>(() => ({
  browser: null,
  yandexMusic: null,
}))

export function controlBrowserMedia(id: BrowserId, action: MediaAction) {
  if (action === 'playPause' || action === 'next' || action === 'previous') {
    postToHost({ type: 'browser', id, command: { action: 'media', control: action } })
  }
}
