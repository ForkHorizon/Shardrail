# Shardrail — исследование UI-технологий (вопрос владельца U8)

> Part of the Shardrail architecture plan. Research input (in Russian) for U8; outcome: owner decision D3 (2026-10-18), AI-authored-code constraint D44. Applied in 02 §8, 08 §6, 10 S5, 11 §3, 12, 13.

*Дата: 2026-10-18. Все цифры взяты из открытых источников в этот день (ссылки ниже). Репозиторий не изменялся. Статус: рекомендация, решение за владельцем.*

**Новая вводная.** Весь код Shardrail пишут ИИ-агенты, Daliys его не пишет. Поэтому главный критерий: насколько надёжно модели пишут на этом стеке. Это складывается из шести факторов:
- объём обучающих данных;
- частота галлюцинаций в API;
- сколько ошибок автоматически ловят компилятор, типы и тесты;
- стабильность API;
- качество документации для ИИ: llms.txt, MCP;
- зрелость экосистемы.

Только после этого идут соответствие продукту и перспективы.

---

## 1. Короткая рекомендация

**Основной выбор: React 19 + TypeScript (strict) + Vite (SPA, без Next.js/RSC).**
- Примитивы интерфейса: **React Aria Components**.
- Виртуализация списков: **TanStack Virtual**.
- Состояние: внешний store, то есть `useSyncExternalStore` или Zustand.
- Markdown при стриминге: потоковый рендер с обязательной санитизацией, например Streamdown + harden/sanitize, обёрнутый в один модуль.
- Типы протокола генерируются из Rust-схем: JSON Schema → TS или `ts-rs`, по D27.

Почему React:
- **Больше всего обучающих данных.** npm: 115 млн загрузок в неделю против 10 млн у Vue и 4 млн у Svelte. Использование в State of JS 2025: 85%.
- **Лучший результат в независимых бенчмарках.** В Web-Bench (ByteDance) у Claude 3.7 pass@2 составил React 65% против Svelte 25% и Vue 30%. В WebCompass 2026 React силён, а Vue стабильно худший.
- **Стабильный API.** React Compiler 1.0 вышел стабильным, React 19 — эволюционное обновление.
- **Сильная доступность из коробки.** React Aria — зрелые доступные примитивы с официальным MCP-сервером и llms.txt.
- **Ошибки ловятся автоматически:** TS-проверка JSX, eslint-плагины `react-hooks`/`react-compiler`/`jsx-a11y`.

**Запасной вариант: Svelte 5 (runes) + Bits UI.**
- Лучшие в индустрии официальные инструменты для ИИ: MCP-сервер с `svelte-autofixer`, ловящим синтаксис Svelte 4, llms.txt и готовый сабагент.
- Самый компактный код: примерно на 20% меньше токенов, чем у React.
- Высокая производительность.
- Минусы:
  - Данных для обучения меньше.
  - В обучающих данных модели много синтаксиса Svelte 4. Это лечится autofixer'ом, но он обязателен.
  - Доступные примитивы беднее, чем у React Aria.
  - Независимые бенчмарки слабее React (кроме SvelteBench, где современные модели дают 100%, но на 9 простых задачах).

**Чего избегать:**
1. **Rust/WASM-UI (Leptos, Dioxus, Yew)** ради общих типов с демоном.
   - Leptos с мая 2026 «lightly maintained / feature-complete».
   - Dioxus ещё до 1.0, ломает API между минорными версиями (0.7 → 0.8 alpha).
   - Yew потерял импульс.
   - Данных для обучения мало, модели путают версии API.
   - Общие типы проще получить кодогенерацией (D27).
2. **Нативные GUI без веба (GPUI, Slint, egui, iced, SwiftUI).** Они ломают требование «тот же UI в браузере и позже на телефоне».
   - Веб-версии у них канвасные, недоступны скринридерам и экспериментальны.
   - SwiftUI работает только на Mac.
3. **SolidJS сейчас.** Выпускается 2.0 (RC.15 от 13.10.2026) с крупными изменениями API (`createResource` → async `createMemo`, `Suspense` → `Loading`, ...). Модели обучены на 1.x, так что риск галлюцинаций максимальный ровно в момент старта проекта.
4. **Vue 3** (стабильно хуже в LLM-бенчмарках; Vapor ещё RC) и **Lit** (самый многословный код, низкая удовлетворённость, мало примитивов).

---

## 2. Сравнительная таблица

Метрики в таблице:
- npm/нед.: загрузки за 10–16.10.2026 по api.npmjs.org.
- ★: звёзды на GitHub на 18.10.2026.
- SoJS 2025: доля использующих / удовлетворённость.
- crates.io: загрузки за последние 90 дней.

| Вариант | Зрелость / последний релиз | Импульс | Надёжность ИИ-кода | Стриминг текста | Доступность (a11y) | Браузер + мобильный | Главный риск | Вердикт |
|---|---|---|---|---|---|---|---|---|
| **React 19** + React Aria + TanStack Virtual | 19.3.1 (14.10.2026); Compiler 1.0 стабилен | npm 115,4 млн/нед.; ★251k; SoJS 85% / 72% | **Наивысшая:** больше всего данных; Web-Bench React 65% (Claude 3.7) против 25–40% у других; MCP + llms.txt у React Aria | Хорошо, если дельты вне React-состояния, склейка по кадру, блочная мемоизация Markdown | **Лучшая:** React Aria (Adobe), 1.22.0 от 09.10.2026 | Да: WKWebView, браузер, Tauri mobile / PWA | Многословность (+25% токенов); легко написать неэффективный ререндер без правил | **Основной** |
| **Svelte 5** (runes) + Bits UI | 5.57.2 | npm 4,1 млн/нед.; ★88,4k; SoJS 27% / 86% | Высокая при обязательном MCP/autofixer; без него модели пишут Svelte 4 | Отлично (компилятор, точечная реактивность) | Средняя: Bits UI (0,8 млн/нед.) моложе React Aria | Да | Смешение Svelte 4/5 в ответах моделей; меньше данных | **Запасной** |
| **Vue 3** (3.5.44; 3.6 Vapor — RC.12) | стабилен | npm 10,3 млн/нед.; ★54,7k; SoJS 52% / 84% | **Низкая** по бенчмаркам: WebCompass — худший, Web-Bench 30–40% | Хорошо | Средняя (Reka UI и др.) | Да | SFC смешивает три синтаксиса — модели ошибаются чаще | Избегать |
| **SolidJS** (1.x стаб.; 2.0 RC.15) | переходный период | npm 4,6 млн/нед. (частично транзитивно); ★36,1k; SoJS 10% / 89% | Низкая сейчас: 2.0 меняет ключевые API, данные в основном 1.x | Отлично (сигналы) | Kobalte | Да | Миграция 1→2 прямо на старте | Избегать до 2.0 + 12 мес. |
| **Preact 11** + signals | 11.0 (29.09.2026) | npm 23,1 млн/нед. (много транзитивного); ★38,9k; SoJS 15% / 81% | Средняя: «как React», но модели путают совместимость | Отлично | Через compat с React-библиотеками (риск) | Да | Слой совместимости, свежий мажор | Нет смысла вместо React |
| **Lit 3** (web components) | 3.3.x | npm 3,95 млн/нед.; ★21,8k; SoJS 10% / 67% | Средне-низкая; самый многословный (+85% токенов к Svelte 5) | Хорошо | Мало готовых доступных примитивов | Да | Бедная экосистема UI | Избегать |
| **Leptos** (Rust/WASM) | 0.8.22 стаб., 0.9 RC0; **lightly maintained** с 05.2026 | crates 1,44 млн за 90 дн.; ★21,4k | Низкая: мало данных, API менялся 0.6→0.7→0.8 | Хорошо, но мост JS↔WASM и размер бандла | Ручная; готовых примитивов почти нет | Браузер да; мобильный через Tauri | Сопровождение одним автором | Избегать |
| **Dioxus** (web/desktop/mobile) | 0.7.10 (30.07.2026), 0.8 alpha | crates 1,22 млн за 90 дн.; ★39,4k | Низкая–средняя: есть llms.txt и MCP, но API ломается каждый минор | Хорошо | Слабая | Да (свои рантаймы) | До 1.0, частые breaking changes | Избегать (следить) |
| **Yew** | 0.23.0 (03.2026) | crates 0,44 млн за 90 дн.; ★32,8k | Низкая | VDOM, средне | Слабая | Браузер | Потерял импульс | Избегать |
| **Dioxus Native / Blitz** | beta, «не production-ready» | ★4,3k (blitz) | Низкая | — | Неполная | Нет браузера в смысле DOM | Нет JS, неполный CSS | Избегать |
| **GPUI** (Zed) | crates.io `gpui` 0.2.2 (10.2025, устарел); веб влит в репозиторий Zed 02.2026; AccessKit 05.2026 | ★91,9k (Zed) | Низкая: мало внешних примеров, API внутренний для Zed | Отлично (GPU) | Только появляется | Веб экспериментальный, мобильного нет | Фреймворк одного продукта | Избегать |
| **Slint** | 1.18.1 (09.2026) | ★24,2k; crates 0,71 млн за 90 дн. | Низкая–средняя: свой DSL `.slint` | Хорошо | Нативная — да; **в вебе скринридеров нет** | Веб — канвас | DSL, a11y в вебе | Избегать |
| **egui / iced** | egui 0.37 (10.2026); iced 0.14 (12.2025) | ★31,6k / 31,8k | Средняя (egui популярен), но immediate mode — не про сложный документный UI | Хорошо | egui: AccessKit на Win/mac, в вебе экспериментально | Веб — канвас; мобильный слабый | Не подходит для текстов и диффов уровня продукта | Избегать |
| **SwiftUI** (только macOS) | зрелый | — | Средняя | Отлично | Отлично | **Нет браузера и Android** | Ломает требование | Избегать |

---

## 3. Заметки по вариантам (с источниками)

### React 19 (+ React Aria, shadcn/Radix, TanStack Virtual)

**Версии и импульс**
- React 19.3.1 вышел 14.10.2026. React 19.2 — 01.10.2025.
- React Compiler 1.0 стабилен с 07.10.2025.
- Источники: [react.dev/blog React 19.2](https://react.dev/blog/2025/10/01/react-19-2), [React Compiler 1.0](https://react.dev/blog/2025/10/07/react-compiler-1), [npm react](https://www.npmjs.com/package/react).
- npm: 115 387 152 загрузки за неделю 10–16.10.2026 ([api.npmjs.org](https://api.npmjs.org/downloads/point/last-week/react), [npmcharts](https://www.npmcharts.com/compare/react,vue,@angular/core,ember-cli,svelte,angular?interval=7)). Репозиторий [react/react](https://github.com/react/react), ★251k.
- State of JS 2025: использование 85%, удовлетворённость 72%. Удовлетворённость ниже, чем у Svelte и Solid, но это мнение людей, а не точность моделей ([2025.stateofjs.com](https://2025.stateofjs.com/en-US/libraries/front-end-frameworks/)).

**Доказательства по ИИ-генерации**
- **Web-Bench** (ByteDance, [arXiv 2505.07473](https://arxiv.org/html/2505.07473), приложение A.1.1): одинаковые 20 задач на React, Vue, Angular и Svelte, pass@2.
  - Claude 3.7: React 65, Vue 30, Angular 40, Svelte 25.
  - Claude 3.7 thinking: 60 / 40 / 50 / 55.
  - GPT-4o: 35 / 30 / 5 / 20.
  - Авторы объясняют разрыв в первую очередь объёмом обучающих данных React.
  - Ограничения: модели 2025 года, по одному проекту на фреймворк.
  - Там же: в Next.js модели путаются в Server/Client Components и ломающихся API (React Router v7). Это аргумент **против Next.js/RSC** и за простую SPA.
- **WebCompass** ([arXiv 2604.18224](https://arxiv.org/html/2604.18224), 2026): модели GPT-5.2, Gemini 3 Pro, Claude Opus 4.5, Qwen3-VL; фреймворки React, Vue и vanilla.
  - Vue стабильно худший.
  - В repair-задачах React лучший (GPT-5.2), что объясняется явными границами компонентов.
- **Токены** ([HackerNoon](https://hackernoon.com/we-measured-the-llm-token-cost-of-5-frontend-frameworks-angular-costs-38percent-more-than-svelte), [gist component-party](https://gist.github.com/ryanatkn/1a2ad5f0988e48945b783fa9c4767c67)): Svelte ×1,00, Vue ×1,115, Solid ×1,196, React ×1,254. Это цена контекста, а не ошибок.

**Доступные примитивы**
- **React Aria Components** 1.22.0 от 09.10.2026 ([релизы](https://github.com/adobe/react-spectrum/releases)), 2,8 млн загрузок в неделю.
- Официальный MCP `@react-aria/mcp` и [llms.txt](https://react-aria.adobe.com/llms.txt) ([react-aria.adobe.com/ai](https://react-aria.adobe.com/ai)).
- Для Shardrail нужны listbox, grid, tree (файловое дерево), dialog (approval) и полная поддержка клавиатуры — React Aria даёт это без самописных ARIA-атрибутов. ИИ особенно плохо пишет такие атрибуты вручную.

**shadcn/ui и Radix**
- shadcn/ui: ★125,9k; `@radix-ui/react-dialog`: 46 млн загрузок в неделю.
- Модели знают этот стек лучше всего, это его плюс.
- Но shadcn — копируемый код поверх Radix. Получаются два набора примитивов, а гарантии a11y у React Aria строже.
- Правило: **один слой примитивов — React Aria.** Внешний вид собирать на Tailwind (137 млн загрузок в неделю), без смешения с Radix.

**Виртуализация и стриминг**
- TanStack Virtual: `@tanstack/react-virtual` 16,4 млн загрузок в неделю, ★7,2k.
- [Streamdown](https://github.com/vercel/streamdown) (Vercel): 4,4 млн загрузок в неделю, ★5,7k.
  - Это замена react-markdown для ИИ-стриминга: незакрытый Markdown и мемоизация по блокам.
  - Встроены [link safety и hardening](https://streamdown.ai/docs/security): разрешённые протоколы и префиксы, запрет data-изображений.
  - Для Shardrail это кандидат на модуль рендера недоверенного текста.
  - Конфигурацию обязательно зафиксировать; решение подтвердить в S5.

**Риски React**
- Неэффективные ререндеры при потоке токенов. Лечится правилами из §5: дельты вне React-состояния, склейка по `requestAnimationFrame`, внешний store, Compiler.
- Многословность.

### Svelte 5 (runes)

- 5.57.2, npm 4,14 млн загрузок в неделю, ★88,4k. SoJS 2025: 27% / 86%. Stack Overflow 2025: «admired» 62,4% ([сводка](https://gist.github.com/tkrotoff/b1caa4c3a185629299ec234d2314e190)).
- **Лучший в индустрии официальный ИИ-тулинг:**
  - [MCP-сервер и CLI](https://svelte.dev/docs/ai/cli) с `svelte-autofixer`. Он ловит синтаксис Svelte 4 (`on:click`, `export let`, `<slot>`), `$effect` вместо `$derived` и пропущенные ключи в `{#each}`.
  - Сабагент `svelte-file-editor`.
  - [llms.txt / llms-full / small](https://svelte.dev/docs/llms).
  - Это прямое признание проблемы: модели генерируют Svelte 4.
- [SvelteBench](https://khromov.github.io/svelte-bench/): большинство фронтирных моделей 2026 года набирают 100% на 9 задачах Svelte 5, слабые модели — 85–91% ([SvelteBench 2.0](https://sveltejs.github.io/ai/)). Задачи маленькие, на уровне компонентов.
- Минусы для Shardrail:
  - Меньше обучающих данных.
  - Слабее экосистема доступных примитивов: Bits UI 0,8 млн загрузок в неделю против React Aria 2,8 млн.
  - Web-Bench на старых моделях хуже React.
- Когда выбрать Svelte: если S5 покажет, что React не укладывается в B3/B4/B5 в WKWebView. Это маловероятно при соблюдении правил стриминга.

### Vue 3
- Стабильная 3.5.44. Vue 3.6 с Vapor Mode — RC.12, нестабильна ([releasealert](https://releasealert.dev/npmjs/_/vue)).
- npm 10,3 млн загрузок в неделю, ★54,7k, SoJS 52% / 84%.
- Худший из протестированных в WebCompass; в Web-Bench 30–40% против 60–65% у React.
- Авторы WebCompass объясняют это SFC, где перемешаны три синтаксиса.
- Для команды из ИИ-агентов это прямой минус.

### SolidJS
- 1.x стабилен. **2.0.0-rc.15 вышел 13.10.2026**, API заморожен, но релиза ещё нет ([релизы](https://github.com/solidjs/solid/releases), [Solid 2.0 RC](https://www.solidjs.com/blog/solid-2-0-rc-the-big-reveal)).
- [MIGRATION.md](https://github.com/solidjs/solid/blob/next/documentation/solid-2.0/MIGRATION.md): `createResource` → async `createMemo` + `<Loading>`, `Suspense` → `Loading`, микротасковый батчинг с `flush()`, разделение `createEffect`, `onMount` → `onSettled`.
- Удовлетворённость самая высокая пятый год подряд (89%), использование 10%.
- Производительность сигналов отличная. Но для ИИ-авторов это худшее время входа: модели знают 1.x, проект стартует на 2.x.

### Preact 11 / signals
- Preact 11 вышел 29.09.2026 ([блог](https://preactjs.com/blog/preact-11/)). 23 млн загрузок в неделю, в основном транзитивных. `@preact/signals`: 1,6 млн.
- Выигрыш в размере неважен для десктопного Tauri.
- React-библиотеки (React Aria) работают через compat-слой — лишний источник ошибок.
- Смысла нет.

### Lit 3 / web components
- Lit 3.3.x, Labs-пакеты (signals, virtualizer) ещё в разработке ([теги](https://github.com/lit/lit/tags)).
- SoJS: 10% / 67%. Самый многословный в замере токенов (+85% к Svelte 5).
- Мало готовых доступных составных виджетов (tree, grid).

### Rust/WASM: Leptos, Dioxus, Yew
- **Leptos:** 0.8.22 от 05.10.2026, 0.9.0-rc0 от 15.10.2026 ([crates.io](https://crates.io/crates/leptos)).
  - Автор в [Status Update May 2026](https://github.com/leptos-rs/leptos/issues/4707): «not abandoned but will be lightly maintained… feature-complete… open to additional maintainers».
  - Для продукта на годы это риск фактора автобуса (bus factor).
- **Dioxus:** 0.7.10 от 30.07.2026, 0.8.0-alpha.1 ([теги](https://github.com/DioxusLabs/dioxus/tags)).
  - Есть официальный llms.txt и MCP ([0.7 release](https://dioxuslabs.com/blog/release-070/)).
  - Но [миграция 0.6→0.7](https://dioxuslabs.com/learn/0.7/migration/to_07/) снова ломающая.
  - Сообщество выпускает «knowledge patch» скиллы именно потому, что модели генерируют устаревший API ([пример](https://playbooks.com/skills/nevaberry/nevaberry-plugins/dioxus-knowledge-patch)).
- **Yew:** 0.23.0 (03.2026). По сравнению с Leptos и Dioxus потерял импульс ([обзор](https://rustify.rs/articles/leptos-vs-dioxus-rust-frontend-2026)).
- **Что даёт и чего стоит общий язык с демоном.**
  - Плюс: компилятор Rust ловит больше ошибок, чем TS.
  - Минусы:
    - мало обучающих данных по этим фреймворкам;
    - мало готовых доступных компонентов (дерево, grid, диалоги — писать самим, а ИИ пишет ARIA плохо);
    - нет зрелых аналогов Streamdown или diff-вьюеров;
    - размер WASM;
    - мост к DOM.
  - Общие типы достигаются дешевле кодогенерацией: D27 уже требует схему журнала со сгенерированными типами Rust и TS.
    - [ts-rs](https://github.com/Aleph-Alpha/ts-rs) 12.0.1 стабилен, 8,1 млн загрузок за 90 дней.
    - Либо `schemars` → JSON Schema → TS.

### Нативные GUI без веба
- **GPUI.**
  - На crates.io `gpui` 0.2.2 (10.2025) устарел. Сообщество выпускает форки (`gpui-standalone`, `gpui-pre-web`).
  - Веб-платформа [влита в Zed 26.02.2026](https://github.com/zed-industries/zed/pull/50228), [AccessKit — 05.2026](https://github.com/zed-industries/zed/pull/56065).
  - Фреймворк одного продукта, внешних примеров мало, для ИИ-агентов неудобен.
- **Dioxus Native / Blitz:** beta, «not production-ready», CSS неполный, JS нет, мобильная версия экспериментальна ([blitz](https://github.com/dioxuslabs/blitz/)).
- **Slint** 1.18.1: веб через канвас, [скринридеры в вебе недоступны](https://docs.slint.dev/latest/docs/slint/guide/platforms/web/). [Royalty-free лицензия](https://github.com/slint-ui/slint/blob/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md) с атрибуцией. Свой DSL — меньше данных для моделей.
- **egui / iced:** immediate mode и канвас. У egui AccessKit на Win/mac, в вебе доступность экспериментальна ([egui](https://github.com/emilk/egui)). Для длинных текстовых логов, диффов и выделения текста это неудобно.
- **SwiftUI:** отлично на Mac, но ломает требования «браузер» и «Android».
- **Вывод.** Все эти варианты ломают D2/D3 (один UI для десктопа, браузера и мобильного) и требуют второго UI для браузера.

### Новые и перспективные (2026)
- **React Compiler 1.0:** автоматическая мемоизация убирает класс ошибок с `useMemo`/`useCallback`, которые часто делают модели. Включить.
- **Vue Vapor / Solid 2 / Dioxus 0.8 / Leptos 0.9:** все в RC или alpha. Следить, на старте не брать.
- **Официальные ИИ-инструменты фреймворков** (Svelte MCP, React Aria MCP, Dioxus llms.txt) — новый критерий выбора. По этому критерию сейчас лидирует Svelte, React Aria на втором месте.

### Особенности Tauri
- Tauri 2.12.4 (crate, 14.10.2026), ★112k, `@tauri-apps/api` 1,96 млн загрузок в неделю ([релизы](https://v2.tauri.app/release/)).
- Мобильная версия работает: есть приложения в сторах. Но отладка и нативные плагины трудны ([r/tauri: опыт](https://www.reddit.com/r/tauri/comments/1nhhomo/launched_ios_and_android_run_coaching_app_with/), [критика](https://www.reddit.com/r/tauri/comments/1maezph/tauri_is_unsuitable_for_mobile/)). D28 правильно откладывает выбор мобильного подхода; любой веб-фреймворк одинаково переносим.
- **Генерация типов.**
  - `tauri-specta` / `specta` годами в RC (2.0.0-rc.25, 05.2026).
  - Главное: по D16 Shardrail использует один проводной протокол демона, а не Tauri-команды. Поэтому tauri-specta почти не нужен.
  - Нужен генератор из Rust-схем протокола и журнала: `ts-rs` (стабилен) или `schemars` → JSON Schema → TS. Сгенерированные файлы закоммичены и проверяются в CI на дрейф.
- WKWebView — это движок WebKit, поэтому e2e-тесты надо гонять в **Playwright WebKit**, а не только в Chromium.

---

## 4. Что поменять в плане (предложение, не решение)
- **U8:** рекомендация «React + React Aria» (как в ревью 15) подтверждается и усиливается критерием «код пишет ИИ». Svelte 5 — запасной вариант с конкретным условием перехода: провал B3/B4/B5 в S5.
- **D3:** уточнить формулировку — «TypeScript strict, React 19 SPA на Vite, без SSR/RSC; один слой примитивов (React Aria)».
- **S5:** добавить прототип конвейера стриминга на React в WKWebView: несколько агентов, 50–100 токенов/с каждый, блочный Markdown, виртуализированная история. Мерить B4/B5 и следы ререндеров.
- **D27:** зафиксировать генератор TS-типов (ts-rs или JSON Schema) и CI-проверку дрейфа.
- Новые риски для 13:
  - «ИИ-код в UI с недоверенным текстом» — смягчается правилами §5.
  - «Дрейф версий фреймворка относительно обучающих данных моделей» — смягчается пинами, llms.txt и MCP.

---

## 5. Правила для ИИ-агентов (для выбранного стека)

1. **Пины и контекст.**
   - Точные версии в `package.json` и lockfile.
   - `AGENTS.md` с таблицей «версия → ссылка на llms.txt/MCP» для React, React Aria, TanStack Virtual, Vite, Vitest и Playwright.
   - Агенты обязаны сверяться с документацией (React Aria MCP, llms.txt), а не полагаться на память.
2. **Новая зависимость только через ADR.**
   - Агент не добавляет пакеты сам.
   - Список разрешённых пакетов ведётся в репозитории.
   - Проверки в CI: `npm audit signatures` и лицензии.
3. **Типы:**
   - Флаги TS: `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`.
   - Запрещены `any`, `as unknown as` и `@ts-ignore`; `@ts-expect-error` — только с комментарием и тестом.
   - Протокол и журнал — только сгенерированные из Rust типы. Ручное редактирование сгенерированных файлов запрещено. CI проверяет дрейф.
4. **Только современный React:**
   - Функциональные компоненты, React 19 API, включённый React Compiler.
   - Без классов, без `forwardRef` (в React 19 не нужен).
   - Без Next.js, RSC и серверных функций: чистая SPA на Vite.
5. **Один слой примитивов.** Все интерактивные элементы (кнопки, меню, listbox, tree, grid, dialog, tooltip) берутся из React Aria Components. Самописные `role`/`aria-*` и `onKeyDown`-навигация запрещены без ревью. Radix/shadcn не смешивать.
6. **Стриминг:**
   - Токенные дельты никогда не проходят через `useState` на каждый токен.
   - Дельты пишутся во внешний store (`useSyncExternalStore`) и склеиваются по `requestAnimationFrame`.
   - Markdown мемоизируется по блокам.
   - Длинные списки и история — только через TanStack Virtual.
   - Есть тест «N агентов × M токенов/с», проверяющий бюджет B4.
7. **Недоверенный текст (06 §5, D22, D38):**
   - Рендер агентского текста только через один модуль `untrusted-render` (Streamdown или аналог) с санитизацией и harden: без raw HTML, data-изображений и удалённых изображений; ссылки по allowlist протоколов с подтверждением перехода.
   - ESLint запрещает `dangerouslySetInnerHTML`, `innerHTML` и `eval` вне этого модуля.
   - Агентский текст не попадает в `href`/`src`/`style` без валидатора.
   - Bidi- и zero-width-символы в командах approval показываются видимыми.
   - CSP + Tauri isolation pattern + минимальные capabilities.
8. **Линтеры как ворота:**
   - ESLint (flat) с `react-hooks`, `react-compiler`, `jsx-a11y`, `no-restricted-syntax` для запретов выше.
   - Prettier, `tsc --noEmit`.
   - Без зелёного CI коммит не принимается.
9. **Тесты:**
   - Vitest + Testing Library на компоненты.
   - **Playwright с движком WebKit** на e2e.
   - axe-core на каждый экран.
   - Фикстуры событий из fake agent (E1.2), а не моки протокола.
   - Тесты рендеринга недоверенного текста (T1.10): bidi, скрытые символы, «безопасный» заголовок над опасной командой.
10. **Состояние:** демон — источник истины (D3). Клиент держит ограниченную проекцию, второго состояния (Redux и т.п.) нет. Для локального UI-состояния — React state или один Zustand-store.
11. **Ревью:** каждый PR от ИИ-агента проверяет второй агент другого семейства моделей. Обязательный чеклист: a11y, недоверенный текст, ререндеры.
12. **Свежесть:** раз в фазу агент сверяет версии и ломающие изменения по changelog и llms.txt, обновляет `AGENTS.md` и фиксирует дату проверки (как в 14).

---

*Ограничения исследования.*
- Публичных бенчмарков по качеству LLM-кода для Solid, Lit, Leptos и Dioxus нет. Выводы по ним основаны на объёме данных, стабильности API и косвенных признаках (knowledge-patch скиллы, официальные autofixer'ы).
- Web-Bench использует модели 2025 года.
- npm-загрузки Preact и Solid завышены транзитивными зависимостями.
