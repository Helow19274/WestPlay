import type { HostMessage } from '../../../shared/host'
import type { BrowserId } from '../../../shared/webApps'
import { useBrowserMedia } from './media'

type BrowserState = Extract<HostMessage, { type: 'browserState' }>
type Session = { current: BrowserState; started: boolean; zoom: number }
const sessions = new Map<BrowserId, Session>()

export function sessionFor(id: BrowserId): Session {
  let session = sessions.get(id)
  if (!session) {
    session = {
      current: {
        type: 'browserState',
        id,
        url: '',
        title: '',
        loading: false,
        back: false,
        forward: false,
        error: null,
      },
      started: false,
      zoom: 1,
    }
    sessions.set(id, session)
  }
  return session
}

/** Keep the last address, but forget everything owned by the destroyed native control. */
export function resetSession(id: BrowserId): void {
  useBrowserMedia.setState({ [id]: null })
  const session = sessionFor(id)
  session.started = false
  session.zoom = 1
  session.current = {
    ...session.current,
    title: '',
    loading: false,
    back: false,
    forward: false,
    error: null,
    media: null,
  }
}

/** Includes browsers whose React screen is currently unmounted. */
export function resetBrowserSessions(): void {
  useBrowserMedia.setState({ browser: null, yandexMusic: null })
  for (const id of sessions.keys()) resetSession(id)
}
