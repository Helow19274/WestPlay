import { useEffect, useRef, useState } from 'react'
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
  url: '',
  title: '',
  loading: false,
  back: false,
  forward: false,
  error: null,
}
const STORAGE = 'westplay.browser.url'
let current = INITIAL
let started = false
let currentZoom = 1
const command = (command: BrowserCommand) => postToHost({ type: 'browser', command })
function savedAddress() {
  try {
    return localStorage.getItem(STORAGE) || 'https://www.google.com/'
  } catch {
    return 'https://www.google.com/'
  }
}

/** The browser surface is a separate native WebView. This page owns only its controls. */
export function BrowserApp() {
  const language = useLanguage()
  const tr = language === 'tr'
  const [state, setState] = useState(current)
  const [address, setAddress] = useState(current.url || savedAddress())
  const [zoom, setZoom] = useState(currentZoom)
  const [generation, setGeneration] = useState(0)
  const area = useRef<HTMLDivElement>(null)
  const handedOver = useCabin((state) => state.truckScreen)
  const remoteId = 'browser-page'

  useEffect(
    () =>
      subscribeHostMessages((message) => {
        if (message.type === 'browserState') {
          current = message
          setState(message)
          if (message.url) {
            if (useKeyboard.getState().field?.id !== 'browser-address') setAddress(message.url)
            try {
              localStorage.setItem(STORAGE, message.url)
            } catch {
              /* Storage may be unavailable. */
            }
          }
        }
        if (message.type === 'browserFocus' && !handedOver) {
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
    [language, handedOver],
  )

  useEffect(() => {
    if (!isInHost() || handedOver) return
    let gone = false
    let frame = 0
    let last = ''
    // Native surfaces cannot participate in the DOM's opening animation.
    whenTransitionEnds(() => {
      if (gone) return
      if (!started) {
        started = true
        command({ action: 'navigate', url: current.url || savedAddress() })
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
  }, [handedOver, generation])

  const navigate = () => {
    useKeyboard.getState().detach('browser-address')
    useKeyboard.getState().detach(remoteId)
    command({ action: 'navigate', url: address })
  }
  const changeZoom = (delta: number) => {
    const factor = Math.max(0.5, Math.min(2, Math.round((zoom + delta) * 10) / 10))
    setZoom(factor)
    currentZoom = factor
    command({ action: 'zoom', factor })
  }
  return (
    <section className="app-panel browser-app" aria-label={tr ? 'Tarayıcı' : 'Browser'}>
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
        />
        <button onClick={navigate}>{tr ? 'Git' : 'Go'}</button>
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
            started = false
            currentZoom = 1
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
