import './yandexMusic.css'

/** The native host grants this iframe permission through AdditionalAllowedFrameAncestors. */
export function YandexMusicApp() {
  return (
    <section className="app-panel yandex-music-app" aria-label="Яндекс Музыка">
      <iframe
        name="yandex-music"
        title="Яндекс Музыка"
        src="https://music.yandex.ru/"
        allow="autoplay; encrypted-media; fullscreen"
        allowFullScreen
      />
    </section>
  )
}
