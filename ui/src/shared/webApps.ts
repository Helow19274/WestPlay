/** Fixed web apps; adding entries is a source change, not a runtime setting. */
export const WEB_APPS = {
  yandexMusic: { url: 'https://music.yandex.ru/', name: 'Яндекс Музыка' },
} as const

export type WebAppId = keyof typeof WEB_APPS
export type BrowserId = 'browser' | WebAppId
