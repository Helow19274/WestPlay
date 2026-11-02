/// <reference types="node" />
import { readFileSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
import { describe, expect, it } from 'vitest'
import { browserSource, useBrowserMedia } from './media'
import { resetBrowserSessions, resetSession } from './session'

const script = readFileSync(new URL('../../../../../crates/host/src/site_media.js', import.meta.url), 'utf8')
function page() {
  class Media {
    paused = true
    ended = false
    isConnected = true
    currentTime = 0
    async play() {
      this.paused = false
    }
    pause() {
      this.paused = true
    }
  }
  const elements: Media[] = []
  const session = {
    metadata: null as any,
    playbackState: 'none',
    setActionHandler(_action: string, _handler: unknown) {},
  }
  const window = {} as any
  runInNewContext(script, {
    window,
    navigator: { mediaSession: session },
    HTMLMediaElement: Media,
    document: { title: 'Page title', querySelectorAll: () => elements },
    location: { href: 'https://example.org/player', hostname: 'example.org' },
    URL,
  })
  return { api: window.westplaySiteMedia, session, elements, Media }
}

describe('browser media bridge', () => {
  it('ignores unstarted media, controls HTML playback, and clears removed media', async () => {
    const { api, elements, Media } = page()
    const video = new Media()
    elements.push(video)
    expect(api.read()).toBeNull()
    await video.play()
    expect(api.read()).toMatchObject({ title: 'Page title', playing: true, playPause: true, next: false })
    await api.control('playPause')
    expect(api.read().playing).toBe(false)
    await api.control('playPause')
    expect(video.paused).toBe(false)
    video.isConnected = false
    elements.length = 0
    expect(api.read()).toBeNull()
  })

  it('tracks detached Audio instances and handles playback rejection', async () => {
    const { api, Media, session } = page()
    const audio = new Media()
    audio.isConnected = false
    await audio.play()
    expect(api.read().playing).toBe(true)
    session.setActionHandler('pause', () => Promise.reject(new Error('site rejected')))
    await expect(api.control('playPause')).resolves.toBeUndefined()
    expect(api.read().playing).toBe(true)
    audio.ended = true
    expect(api.read()).toBeNull()
  })

  it('uses site metadata and registered actions, including handler removal', async () => {
    const { api, session } = page()
    session.metadata = { title: 'Song', artist: 'Artist', album: 'Album', artwork: [{ src: '/cover.jpg' }] }
    session.playbackState = 'playing'
    let next = 0
    session.setActionHandler('nexttrack', () => {
      next++
    })
    expect(api.read()).toMatchObject({
      title: 'Song',
      artwork: 'https://example.org/cover.jpg',
      next: true,
      playPause: false,
    })
    await api.control('next')
    expect(next).toBe(1)
    session.setActionHandler('nexttrack', null)
    await api.control('next')
    expect(next).toBe(1)
    expect(api.read().next).toBe(false)
    session.metadata.artwork = [{ src: 'javascript:alert(1)' }]
    expect(api.read().artwork).toBeNull()
  })

  it('keeps sources isolated and clears them on close and game exit', () => {
    const { api, session } = page()
    session.metadata = { title: 'Song' }
    session.playbackState = 'paused'
    useBrowserMedia.setState({
      browser: browserSource('browser', api.read()),
      yandexMusic: browserSource('yandexMusic', api.read()),
    })
    resetSession('browser')
    expect(useBrowserMedia.getState().browser).toBeNull()
    expect(useBrowserMedia.getState().yandexMusic?.title).toBe('Song')
    resetBrowserSessions()
    expect(useBrowserMedia.getState().yandexMusic).toBeNull()
    expect(browserSource('browser', undefined)).toBeNull()
  })
})
