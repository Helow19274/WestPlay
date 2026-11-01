import { useCallback, useEffect, useRef, useState } from 'react'
import { WEB_APPS, type BrowserId } from '../../../shared/webApps'
import {
  isInHost,
  postToHost,
  subscribeHostMessages,
  type BrowserCommand,
  type HostMessage,
} from '../../../shared/host'
import { TextField } from '../../keyboard/TextField'
import { useKeyboard } from '../../keyboard/store'
import { useCabin } from '../../state/store'
import { useLanguage } from '../../state/text'
import { whenTransitionEnds } from '../../state/launch'
import './browser.css'

type BrowserState = Extract<HostMessage, { type: 'browserState' }>
const INITIAL: BrowserState = {
  type: 'browserState',
  id: 'browser',
  url: '',
  title: '',
  loading: false,
  back: false,
  forward: false,
  error: null,
}
const STORAGE = 'westplay.browser.url'
type Session = { current: BrowserState; started: boolean; zoom: number }
const sessions = new Map<BrowserId, Session>()
function sessionFor(id: BrowserId): Session {
  let session = sessions.get(id)
  if (!session) {
    session = { current: { ...INITIAL, id }, started: false, zoom: 1 }
    sessions.set(id, session)
  }
  return session
}
function savedAddress() {
  try {
    return localStorage.getItem(STORAGE) || 'https://www.google.com/'
  } catch {
    return 'https://www.google.com/'
  }
}

/** The browser surface is a separate native WebView. This page owns only its controls. */
export function BrowserApp({ id = 'browser' }: { id?: BrowserId }) {
  const preset = id === 'browser' ? undefined : WEB_APPS[id]
  const session = sessionFor(id)
  const command = useCallback((command: BrowserCommand) => postToHost({ type: 'browser', id, command }), [id])
  const language = useLanguage()
  const tr = language === 'tr'
  const [state, setState] = useState(session.current)
  const [address, setAddress] = useState(session.current.url || preset?.url || savedAddress())
  const [zoom, setZoom] = useState(session.zoom)
  const [generation, setGeneration] = useState(0)
  const area = useRef<HTMLDivElement>(null)
  const handedOver = useCabin((state) => state.truckScreen)
  const remoteId = `browser-page:${id}`

  useEffect(
    () =>
      subscribeHostMessages((message) => {
        if (message.type === 'browserState' && message.id === id) {
          session.current = message
          setState(message)
          if (message.url) {
            if (useKeyboard.getState().field?.id !== 'browser-address') setAddress(message.url)
            try {
              if (!preset) localStorage.setItem(STORAGE, message.url)
            } catch {
              /* Storage may be unavailable. */
            }
          }
        }
        if (message.type === 'browserFocus' && message.id === id && !handedOver) {
          if (message.editable) {
            useKeyboard.getState().attach(
              {
                id: remoteId,
                returnKind: 'done',
                read: () => '',
                write: () => {},
                submit: () => command({ action: 'input', event: { key: 'enter' } }),
                send: (event) => command({ action: 'input', event }),
              },
              language,
            )
          } else useKeyboard.getState().detach(remoteId)
        }
      }),
    [language, handedOver, id, session, preset, command, remoteId],
  )

  useEffect(() => {
    if (!isInHost() || handedOver) return
    let gone = false
    let frame = 0
    let last = ''
    // Native surfaces cannot participate in the DOM's opening animation.
    whenTransitionEnds(() => {
      if (gone) return
      if (!session.started) {
        session.started = true
        command({ action: 'navigate', url: preset?.url || session.current.url || savedAddress() })
      }
      const place = () => {
        if (gone) return
        const box = area.current?.getBoundingClientRect()
        const cabin = document.querySelector('.carplay')?.getBoundingClientRect()
        const keyboard = document.querySelector('.keyboard.shown')?.getBoundingClientRect()
        if (box && cabin) {
          const scale = window.devicePixelRatio
          const left = Math.max(0, box.left, cabin.left)
          const top = Math.max(0, box.top, cabin.top)
          const right = Math.min(box.right, cabin.right, window.innerWidth)
          const bottom = Math.min(box.bottom, cabin.bottom, window.innerHeight, keyboard?.top ?? Infinity)
          const rect = {
            x: Math.round(left * scale),
            y: Math.round(top * scale),
            width: Math.max(0, Math.floor((right - left) * scale)),
            height: Math.max(0, Math.floor((bottom - top) * scale)),
          }
          const next = JSON.stringify(rect)
          if (last !== next) {
            last = next
            command(rect.width && rect.height ? { action: 'show', ...rect } : { action: 'hide' })
          }
        }
        frame = requestAnimationFrame(place)
      }
      place()
    })
    return () => {
      gone = true
      cancelAnimationFrame(frame)
      command({ action: 'hide' })
      useKeyboard.getState().detach(remoteId)
    }
  }, [handedOver, generation, session, preset, command, remoteId])

  const navigate = () => {
    useKeyboard.getState().detach('browser-address')
    useKeyboard.getState().detach(remoteId)
    command({ action: 'navigate', url: preset?.url || address })
  }
  const changeZoom = (delta: number) => {
    const factor = Math.max(0.5, Math.min(2, Math.round((zoom + delta) * 10) / 10))
    setZoom(factor)
    session.zoom = factor
    command({ action: 'zoom', factor })
  }
  return (
    <section className="app-panel browser-app" aria-label={preset?.name || (tr ? 'Tarayıcı' : 'Browser')}>
      <div className="browser-toolbar">
        <button aria-label={tr ? 'Geri' : 'Back'} disabled={!state.back} onClick={() => command({ action: 'back' })}>
          ←
        </button>
        <button
          aria-label={tr ? 'İleri' : 'Forward'}
          disabled={!state.forward}
          onClick={() => command({ action: 'forward' })}
        >
          →
        </button>
        <button
          aria-label={state.loading ? 'Stop' : 'Reload'}
          onClick={() => command({ action: state.loading ? 'stop' : 'reload' })}
        >
          {state.loading ? '×' : '↻'}
        </button>
        <TextField
          id="browser-address"
          value={address}
          onChange={setAddress}
          onSubmit={navigate}
          returnKind="go"
          hint="https://…"
          label={tr ? 'Web adresi' : 'Website address'}
          readOnly={!!preset}
        />
        <button onClick={navigate}>{preset ? (tr ? 'Başlangıç' : 'Start page') : tr ? 'Git' : 'Go'}</button>
      </div>
      <div className="browser-info">
        <span title={state.url}>
          {state.loading ? (tr ? 'Yükleniyor…' : 'Loading…') : state.title || (tr ? 'Tarayıcı' : 'Browser')}
        </span>
        <button aria-label="Zoom out" disabled={zoom <= 0.5} onClick={() => changeZoom(-0.1)}>
          −
        </button>
        <span className="browser-zoom">{Math.round(zoom * 100)}%</span>
        <button aria-label="Zoom in" disabled={zoom >= 2} onClick={() => changeZoom(0.1)}>
          +
        </button>
        <button
          onClick={() => {
            command({ action: 'close' })
            session.started = false
            session.zoom = 1
            setZoom(1)
            setGeneration((value) => value + 1)
          }}
        >
          {tr ? 'Yeniden başlat' : 'Restart'}
        </button>
      </div>
      {state.error && (
        <div className="browser-error" role="alert">
          {state.error}
        </div>
      )}
      <div ref={area} className="browser-surface">
        {!isInHost() && (
          <p>{tr ? 'Web siteleri WestPlay uygulamasında açılır.' : 'Websites open in the WestPlay app.'}</p>
        )}
      </div>
    </section>
  )
}
