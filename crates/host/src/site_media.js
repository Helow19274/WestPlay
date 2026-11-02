// Installed before the site's scripts. The host only polls data and sends fixed media actions;
// the untrusted page never gets access to the cabin's message bridge.
(() => {
  const session = navigator.mediaSession
  const handlers = new Map()
  if (session) {
    const original = session.setActionHandler.bind(session)
    session.setActionHandler = (action, handler) => {
      original(action, handler)
      if (handler) handlers.set(action, handler)
      else handlers.delete(action)
    }
  }
  let last = null
  let wasConnected = false
  const remember = (media) => {
    last = media
    wasConnected = media.isConnected
  }
  // Some players use new Audio() without inserting it into the document.
  const play = HTMLMediaElement.prototype.play
  HTMLMediaElement.prototype.play = function (...args) {
    remember(this)
    return play.apply(this, args)
  }
  const element = () => {
    const media = [...document.querySelectorAll('audio, video')]
    const active = media.find((m) => !m.paused && !m.ended)
    if (active) remember(active)
    if (last && (last.ended || (wasConnected && !last.isConnected))) last = null
    return active || last || media.find((m) => m.currentTime > 0 && !m.ended) || null
  }
  const playing = (media) => media ? !media.paused && !media.ended : session?.playbackState === 'playing'
  const text = (value) => typeof value === 'string' ? value.slice(0, 2048) : ''
  const artwork = (value) => {
    try {
      const url = new URL(value, location.href)
      return ['https:', 'http:'].includes(url.protocol) ? url.href : null
    } catch { return null }
  }
  window.westplaySiteMedia = {
    read() {
      const media = element()
      const meta = session?.metadata
      if (!media && !(meta && session.playbackState !== 'none')) return null
      const plays = playing(media)
      return {
        title: text(meta?.title) || text(document.title) || location.hostname,
        artist: text(meta?.artist) || location.hostname,
        album: text(meta?.album),
        artwork: meta?.artwork?.length ? artwork(meta.artwork[0].src) : null,
        playing: plays,
        playPause: !!media || handlers.has(plays ? 'pause' : 'play'),
        next: handlers.has('nexttrack'),
        previous: handlers.has('previoustrack'),
      }
    },
    async control(action) {
      const media = element()
      const name = action === 'playPause' ? (playing(media) ? 'pause' : 'play')
        : action === 'next' ? 'nexttrack' : action === 'previous' ? 'previoustrack' : null
      if (!name) return
      try {
        if (handlers.has(name)) await handlers.get(name)({ action: name })
        else if (media && name === 'play') await media.play()
        else if (media && name === 'pause') media.pause()
      } catch { /* A site may reject playback; the next poll reports the actual state. */ }
    },
  }
})()
