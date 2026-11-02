import { useEffect, useRef, useState } from 'react'
import mpegts from 'mpegts.js'
import { isInHost, postToHost, subscribeHostMessages } from '../../../shared/host'
import { useLanguage } from '../../state/text'
import { useCabin } from '../../state/store'
import { appleMusicPlayer } from '../music/pauseWithGame'
import { spotifyPlayer } from '../spotify/pauseWithGame'
import { youtubePlayer } from '../youtube/pauseWithGame'
import { youtubeMusicPlayer } from '../youtubeMusic/pauseWithGame'
import './airplay.css'

const command = (action: 'start' | 'stop') => postToHost({ type: 'airplay', command: { action } })
const players = [appleMusicPlayer, spotifyPlayer, youtubePlayer, youtubeMusicPlayer]

/** The receiver plays audio itself. This first version owns a session only while this app is open. */
export function AirPlayApp() {
  const tr = useLanguage() === 'tr'
  const video = useRef<HTMLVideoElement>(null)
  const [url, setUrl] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  const [starting, setStarting] = useState(false)
  const [playing, setPlaying] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const route = useCabin((state) => state.navigation.route)
  const visible = route.kind === 'app' && route.app === 'airplay'
  const supported = isInHost() && mpegts.getFeatureList().mseLivePlayback

  useEffect(() => {
    const unsubscribe = subscribeHostMessages((message) => {
      if (message.type !== 'airplayState') return
      setStarting(false)
      setRunning(message.running)
      setUrl(message.url)
      setError(message.error)
      if (!message.running) setPlaying(false)
    })
    return () => {
      unsubscribe()
      command('stop')
    }
  }, [])

  useEffect(() => {
    // Outgoing screens remain mounted during animations; stop their external audio immediately.
    if (!visible) command('stop')
  }, [visible])

  useEffect(() => {
    const element = video.current
    if (!element || !url || !visible || !supported) return
    let gone = false
    let player: ReturnType<typeof mpegts.createPlayer> | null = null
    let retry: ReturnType<typeof setTimeout> | undefined
    let generation = 0
    let lastTime = -1
    let movedAt = performance.now()
    const destroy = () => {
      generation++
      if (!player) return
      player.destroy()
      player = null
    }
    const reconnect = () => {
      if (gone || retry) return
      setPlaying(false)
      // Defer destruction: an error may be emitted synchronously from the player's loader.
      retry = setTimeout(() => {
        retry = undefined
        destroy()
        if (!gone) connect()
      }, 1500)
    }
    const connect = () => {
      const connection = ++generation
      const failed = () => {
        if (connection === generation) reconnect()
      }
      lastTime = -1
      movedAt = performance.now()
      setPlaying(false)
      try {
        player = mpegts.createPlayer(
          { type: 'mpegts', isLive: true, url, hasAudio: false, hasVideo: true },
          {
            enableWorker: false,
            enableStashBuffer: false,
            lazyLoad: false,
            liveBufferLatencyChasing: true,
            liveBufferLatencyMaxLatency: 1.5,
            liveBufferLatencyMinRemain: 0.3,
            autoCleanupSourceBuffer: true,
            autoCleanupMaxBackwardDuration: 10,
            autoCleanupMinBackwardDuration: 3,
          },
        )
        player.on(mpegts.Events.ERROR, failed)
        player.on(mpegts.Events.LOADING_COMPLETE, failed)
        player.attachMediaElement(element)
        player.load()
        void element.play().catch(() => {
          if (!gone) failed()
        })
      } catch {
        reconnect()
      }
    }
    const stalled = () => setPlaying(false)
    element.addEventListener('waiting', stalled)
    element.addEventListener('ended', reconnect)
    connect()
    const watch = setInterval(() => {
      if (element.currentTime !== lastTime && element.readyState >= 2) {
        lastTime = element.currentTime
        movedAt = performance.now()
        setPlaying(true)
      } else if (performance.now() - movedAt > 5000) {
        setPlaying(false)
        if (performance.now() - movedAt > 15000) reconnect()
      }
    }, 500)
    return () => {
      gone = true
      clearTimeout(retry)
      clearInterval(watch)
      element.removeEventListener('waiting', stalled)
      element.removeEventListener('ended', reconnect)
      destroy()
    }
  }, [url, visible, supported])

  const start = () => {
    for (const player of players) if (player.playingHere()) player.pauseHere()
    setError(null)
    setStarting(true)
    command('start')
  }
  const stop = () => {
    setUrl(null)
    setPlaying(false)
    command('stop')
  }

  return (
    <section className="app-panel airplay-app" aria-label="AirPlay">
      <header className="airplay-toolbar">
        <strong>AirPlay</strong>
        <span role="status">
          {starting
            ? tr
              ? 'Başlatılıyor…'
              : 'Starting…'
            : playing
              ? tr
                ? 'Ekran yansıtılıyor'
                : 'Screen mirroring'
              : running
                ? tr
                  ? 'iPhone bekleniyor'
                  : 'Waiting for iPhone'
                : tr
                  ? 'Kapalı'
                  : 'Off'}
        </span>
        {running || starting ? (
          <button onClick={stop}>{tr ? 'Durdur' : 'Stop'}</button>
        ) : (
          <button disabled={!supported || !visible} onClick={start}>
            {tr ? 'Başlat' : 'Start'}
          </button>
        )}
      </header>
      <div className="airplay-picture">
        <video
          ref={video}
          autoPlay
          muted
          playsInline
          aria-label="AirPlay video"
          style={{ visibility: playing ? 'visible' : 'hidden' }}
        />
        {!playing && (
          <div className="airplay-waiting">
            <svg viewBox="0 0 100 80" aria-hidden="true">
              <path d="M30 52H12V8h76v44H70M50 40 74 70H26Z" />
            </svg>
            <strong>{tr ? 'iPhone ekranınız, burada.' : 'Your iPhone screen, here.'}</strong>
            <p>
              {!supported
                ? tr
                  ? 'AirPlay, WestPlay uygulamasında desteklenen bir tarayıcı gerektirir.'
                  : 'AirPlay requires a supported browser inside the WestPlay app.'
                : running
                  ? tr
                    ? 'Aynı yerel ağa bağlanın. iPhone’da Denetim Merkezi → Ekran Yansıtma → WestPlay seçin.'
                    : 'Join the same local network. On iPhone, open Control Center → Screen Mirroring → WestPlay.'
                  : tr
                    ? 'Bağlanmak için AirPlay’i başlatın.'
                    : 'Start AirPlay to connect.'}
            </p>
          </div>
        )}
      </div>
      {error && (
        <p className="airplay-error" role="alert">
          {error}
        </p>
      )}
      <footer>
        {tr
          ? 'Ses düzeyini iPhone’dan ayarlayın. Bu ekrandan ayrılmak bağlantıyı keser.'
          : 'Adjust sound on your iPhone. Leaving this screen disconnects AirPlay.'}
      </footer>
    </section>
  )
}
