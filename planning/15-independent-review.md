# Shardrail — независимое ревью архитектурного плана (Stage 0)

**Дата:** 2026-10-18 · **Объект:** `planning/00…14`, `README.md` (коммит `abf835b`, ветка `main`) · **Архив не использовался.**
**Метод:** полное чтение документов + сверка внешних утверждений с первоисточниками (ACP spec, Codex app-server docs, Claude Agent SDK / legal, Hermes docs, Grok Build README/docs, Tauri releases, исходники/PR про macOS process tracking). Ничего не запускалось, репозиторий не изменялся.

---

## 1. Короткий вердикт

План **сильный по мышлению и слабый по исполнимости для одного человека**. Модель угроз, принцип «таймаут ≠ allow», отказ от скрейпинга TUI, safe-Git, честная маркировка «Verified/Unverified» — выше среднего уровня. Но:

1. **Он уже частично устарел** за 3 дня с момента написания: ACP v2 (draft) меняет жизненный цикл prompt/turn, удаляет `session/load`, `fs/*`, `terminal/*`, `session/set_mode`; Codex app-server добавил маршрутизацию approvals на авто-ревьюер (`approvalsReviewer`) и сделал нужные плану методы `experimental`; у Claude SDK изменился дефолтный permission mode и юридическая страница.
2. **Есть внутренние противоречия**, которые бьют по главному инварианту (approvals): ответ на approval не журналируется до отправки; D24 запрещает experimental API, но эскалация cancel в 05 §1 опирается на experimental-метод Codex; Grok выбран «как песочный провайдер для P1a», но его песочница по документации не удовлетворяет границе D20(d) и fail-open.
3. **P1a — не «минимальный срез», а половина продукта** (полная state machine, host protection, code-signing admission, containment helper, exec-denied Git worker с Seatbelt, write lease, desktop shell). Для соло-разработчика это XL+, гейт которого не закроется месяцами.

**Рекомендация:** не утверждать план как есть. Сделать один раунд правок (критичные пункты ниже), резко сузить P1a до «одного ACP-провайдера + журнал + approvals once/deny + diff от провайдера» и разрешить 3–4 спайка (S2, S1-lite, S5, S9-lite) вместо девяти.

---

## 2. Критические находки (по убыванию серьёзности)

### C1. Ответ на approval не журналируется до отправки → нарушение F21/D19 при крэше
**Где:** `05-state-machine.md` §4 (строки `pending → responded_unacked`, `pending | daemon restart → withdrawn(daemon_restart)`), `04-sessions-events-data.md` §4 («Approval responses are tracked by the approval record, not the delivery table»).
**Проблема:** для команд есть intent-журнал (`command_accepted` до side effects, D30), для ответов на approvals — нет. Если демон отправил `allow` провайдеру и упал до коммита `responded_unacked`, после рестарта запись в `pending` → `withdrawn(daemon_restart)`. UI покажет «отозвано», хотя агент получил allow и выполнил команду. Это ровно то, что F21 и D19 запрещают («delivered decisions never shown as withdrawn»).
**Фикс:** ввести состояние `responding(decision)` с правилом «journal → fsync → write to provider stdin». На рестарте `responding` → `delivered_unconfirmed(decision)`, никогда не `withdrawn`. Добавить в E1.7/T1.7 fault-injection тест «kill -9 между write() и commit».

### C2. Протоколы провайдеров ушли вперёд: ACP v2, Codex `approvalsReviewer`, experimental-гейты
**Где:** `03-providers.md` §1–§5, `05` §4 (acknowledgement rules), `12` D7, D24, D33, `14` EV-02.
**Факты (см. §4 fact-check):**
- **ACP v2 опубликован как draft**: `session/prompt` больше не завершает turn (завершение — `state_update: idle` + `stopReason`), `session/load` удалён (теперь `session/resume` + `replayFrom`), `fs/*`, `terminal/*`, `session/set_mode` удалены, `messageId` обязателен, `session/request_permission` получил `title`/`subject` (subject может отсутствовать или быть `command` без `toolCallId`). В v1 тем временем стабилизированы `session/resume`, `session/list`, `session/close`, `session/delete`, elicitation, `$/cancel_request`, `messageId` (опционально).
- Правило подтверждения ACP в 05 §4 («`tool_call_update` для того же `toolCallId` **или ответ `session/prompt`**») в v2 ломается: ответ prompt — это подтверждение вставки, а у permission может не быть `toolCallId`.
- **Codex** `turn/start`/`thread/start` имеют `approvalsReviewer` — approvals можно маршрутизировать на автоматического ревьюера (`auto_review`), и они **никогда не дойдут до Shardrail**. План это не пинит.
- Codex `thread/backgroundTerminals/terminate|list|clean` — **experimental** (требуют `capabilities.experimentalApi`), `osPid` nullable. А D24 запрещает experimental API. При этом 05 §1 (эскалация), 03 §5 (матрица, «Verified») и 03 §6 на них опираются → **внутреннее противоречие**.
- Сам `codex app-server` документирован как experimental и «not supported for production workloads» — это нужно записать как принятый риск R1, а не только «schema drift».
**Фикс:** (a) адаптер ACP проектировать dual-stack v1/v2 сразу (Rust SDK 1.x уже имеет `protocol_router().with_v1().with_v2()`), нормализовать по v2-семантике (state_update) и эмулировать её для v1; (b) переписать ack-правила под v2; (c) пинить `approvalsReviewer: "user"` на каждом `thread/start`/`turn/start` и проверять в ответе; (d) решить явно: либо разрешить узкий allowlist experimental-методов (backgroundTerminals) с записью в D24, либо убрать их из эскалации и полагаться на containment; (e) D7 переформулировать: «не объявлять client `fs`/`terminal` в v1; в v2 их нет».

### C3. Grok как «песочный провайдер для P1a» не удовлетворяет границе D20 и fail-open
**Где:** `06-security.md` §3 (boundary (b),(d),(e)), `10` P1a («prefer a sandboxed one, e.g. Grok»), `13` §3.
**Факты (README `xai-org/grok-build`, docs.x.ai):** профиль `workspace` пишет в `CWD + /tmp + ~/.grok/`; даже `read-only` и `strict` пишут в `~/.grok/`. Пользовательские/проектные профили — `~/.grok/sandbox.toml` и **`.grok/sandbox.toml` в проекте** (т.е. в writable root агента). «If the sandbox cannot be applied … Grok logs a warning and continues without enforcement» — **fail-open**. Сетевые ограничения частичные (seccomp на Linux, на macOS — no-op, план это знает).
**Следствие:** агент Grok в любом профиле может переписать собственный конфиг/allow-правила/профиль песочницы для **следующих** сессий; D20(d) выполняется только через «pinning + hash snapshot», а snapshot в начале/конце сессии ловит это постфактум. Плюс если Seatbelt не применился, хост выглядит «protected», а фактически нет.
**Фикс:** (1) в S2/S7 явно проверять запись в `~/.grok/**` и `.grok/sandbox.toml`; (2) запускать Grok с явным `--sandbox <profile>` и **проверять применение** (парсить stderr/`~/.grok/sandbox-events.jsonl` или делать probe-запись) — при неудаче session refuse; (3) hash-snapshot делать **перед каждым turn**, а не только в начале/конце; (4) пересмотреть ожидание U21: вероятно, ни один провайдер в дефолтном профиле не даст `protected` на macOS — к этому надо готовить продуктовую формулировку заранее.

### C4. P1a не минимален; гейт недостижим для соло-разработчика
**Где:** `10-roadmap-and-backlog.md` §1 (P1a), §3 (E1.1–E1.12), `11` M01–M15.
**Проблема:** P1a включает: полную state machine «со всеми четырьмя видами ячеек», host protection + trust, approvals с reconfirmation, write lease, safe Git с exec-denied воркером (Seatbelt/seccomp) и CI-tripwire, macOS containment helper, peer admission по code-signing identity (а значит Developer ID и подписанные сборки уже в P1a), Tauri shell с menu bar/keychain/notifications, UI system с WCAG. Это 12 эпиков, из которых 6 помечены high-risk. Реальный «первый полезный результат» (увидеть стрим Grok и ответить на approval) отодвигается за всё это.
**Фикс:** см. §6 — P1a′ = журнал + 1 ACP-адаптер + approvals once/deny + diff **из протокола провайдера** (ACP diff content / Codex `turn/diff/updated`) вместо собственного Git; без host-protection claims (просто баннер «approvals not protected»), без write lease (предупреждение), без code-signing admission (dev-сборки и так `unprotected` по 06 §4).

### C5. Апгрейд/рестарт демона = массовое убийство turn'ов; не описано
**Где:** `02` §3 (D11, version skew «restart daemon»), `06` §8 (macOS helper убивает потомков, когда демон уходит), `09` §1 (systemd `KillMode=control-group`), `05` §1 restart rows.
**Проблема:** на macOS helper по замыслу убивает дерево при исчезновении демона; на Linux остановка юнита убивает cgroup. Значит любой апгрейд демона (в т.ч. автообновление shell через Tauri updater, «restart daemon» при version skew) отменяет все работающие turn'ы. Это противоречит духу D11 («agents keep working»), а state machine трактует это как аварию (`killed_by_restart`), а не как плановую процедуру.
**Фикс:** описать **drain-upgrade**: запрет новых turn'ов → ожидание idle/approval-free состояния (с таймаутом и списком затронутых сессий) → рестарт. Обновление shell не должно рестартовать демон, если протокол совместим. Добавить строку в 05 §1 «planned restart» отдельно от crash.

### C6. Approval-карточка строится из «структурных полей», которые тоже пишет модель
**Где:** `06` §5 («Approval cards are built only from structured protocol fields (never from model text…)»), `08` §1.
**Проблема:** ACP `title`/`description` (v2 `title` обязателен), Codex `reason`, `commandActions`, Hermes `description` — это текст, сгенерированный моделью/агентом. Prompt-injection в репозитории может заставить агента написать `title: "Run tests (safe, read-only)"` для `curl … | sh`. «Структурное поле» ≠ «доверенное поле».
**Фикс:** в карточке визуально главным делать только машинно-проверяемое: argv/`rawInput`/`command` + `cwd` + пути из `locations`/`changes` + класс опции; всё «объяснительное» (`title`, `reason`, `description`) — второстепенным блоком с меткой «со слов агента». Если `command` отсутствует (Codex network approvals, ACP permission без subject) — карточка явно говорит «действие не раскрыто провайдером», allow требует второго подтверждения.

### C7. Обходы approvals внутри самих провайдеров не перечислены как вход в модель угроз
**Где:** `06` §3, §11 («provider-internal auto-approvals are invisible»), `03` §6.
**Факты:**
- Claude SDK: allow-правила из settings-файлов **молча** обходят `canUseTool` («allow rules coming from settings files aren't visible to the check»); при неуказанном `permissionMode` SDK ≥ 0.3.286 может стартовать в **`auto`** (классификатор). В `trusted` workspace план разрешает settingSources → пользовательские/проектные allow-правила отключают человеческие approvals.
- Codex: `approvalsReviewer: auto_review`; `tool/requestUserInput` с `autoResolutionMs` (авто-разрешение); network approvals группируются («one prompt unblocks multiple queued requests»).
- Hermes: в ACP маршрутизируются только «dangerous terminal commands» — правки файлов не спрашиваются вообще.
- Grok: `--always-approve`, `_meta.yoloMode`, правила в `~/.grok`.
- ACP: tool kind `switch_mode` и `config_option_update` — агент может сам сменить режим сессии (например, выйти из plan-режима).
**Фикс:** таблица «per provider: какие действия вообще не доходят до approval» в 03 §6 + UI-индикатор на сессию («Hermes: правки файлов без подтверждения»). Всегда явно передавать `permissionMode: "default"` (Claude), `approvalsReviewer: "user"` (Codex). Любой `config_option_update`/mode change от агента — событие политики: журналировать, показывать, при расширении прав — приостанавливать до подтверждения (если провайдер позволяет) или переводить хост в `unprotected`.

### C8. Условия использования касаются не только Claude
**Где:** `13` U9, `09` §2, `03` §6.
**Факты:**
- Codex docs: «App-server authentication has never been permitted for commercial or hosted services» (рекомендуют Sign in with ChatGPT / partner waitlist). Для коммерческого Shardrail это аналог U9, а в плане его нет.
- Claude legal (`code.claude.com/docs/en/legal-and-compliance`) сейчас точнее, чем EV-04: запрещено предлагать claude.ai login **в своём приложении** и собирать/посредничать токены; **разрешено** конечному пользователю входить своей подпиской в **немодифицированный бинарь Claude Code**, в т.ч. когда платформа его хостит. План (D29) эту развилку не различает.
**Фикс:** добавить U25 «Codex app-server в коммерческой сборке», переписать U9 с двумя вариантами (SDK/ACP-адаптер vs немодифицированный `claude` CLI, залогиненный самим пользователем), EV-04 обновить ссылкой на legal-страницу.

### C9. macOS containment: механизм не назван, а очевидный не работает
**Где:** `06` §8, `10` S9, `12` D32.
**Факты:** `NOTE_TRACK`/`NOTE_CHILD` в kqueue возвращают `ENOTSUP` с macOS 10.5 (xnu `filt_procattach`); coalitions и `responsibility_*` требуют entitlement. Рабочие best-effort техники (проверено в Bun): `NOTE_FORK` + скан по `p_puniqueid` + **унаследованный sentinel-fd** с `proc_listpidspath`, плюс process group. Всё это обходится `closefrom()` + setsid + быстрый double-fork.
**Фикс:** записать в 06 §8 конкретную технику и её остаточные дыры; в S9 «done when» добавить тест `fork→exit→setsid→closefrom→sleep`. Честно понизить I2 на macOS до «best effort, residual всегда показывается».

### C10. Модель сессии неполная: нет очереди, закрытия, purge, политики при порче журнала
**Где:** `05` (все разделы), `01` F7/F17, `04` §6–§7.
**Пробелы:**
- **Очередь (F7, сценарий 4):** «pending instruction» не моделируется. Принятый, но ещё не отправленный prompt при рестарте попадёт в `delivery unknown` (05 §5 `accepted → unknown`), хотя он точно не доставлялся. Нужно состояние `queued` (рестарт → `queued`/`discarded`, не `unknown`).
- **Жизненный цикл сессии:** нет измерения `open/closed/archived/purged`; F17 purge при живом процессе не определён; ACP `session/close` (стабилен) не используется.
- **Write lease (I4):** лиз держится, пока жив процесс; idle-сессия с живым процессом блокирует worktree бессрочно. Нужна команда «end session / release lease».
- **Порча журнала / disk full:** что происходит с живыми процессами, когда журнал недоступен на запись? Approvals нельзя ни принять (C1), ни безопасно отклонить без записи. Нужно правило «journal unavailable → все pending approvals отвечаются deny/cancel (fail closed), новые turn'ы запрещены».
- **Shared process (D8):** desync одного stdio ломает **все** сессии процесса, а link-состояние моделируется per-session. `T_cancel` на shared-процессе требует подтверждения пользователя — если клиента нет, turn навсегда в `cancelling`.
- **Desync = авто-kill:** 05 §2 + U22 — любой malformed frame → `desynced` → автотерминация через `T_grace` без клиента. Баг провайдера убивает часовую работу. Предлагаю: без клиента — не убивать, а ждать (orphan-daemon таймер уже есть).

---

## 3. Вторичные находки

| # | Где | Находка | Фикс |
|---|---|---|---|
| S1 | `05` §3, строка «idle + approval request» | Ссылка на D24 неверна (это 03 §2 «unknown requests refused»). В ACP v2 и Codex background-активность может запрашивать permission при foreground `idle` — «refuse immediately» безопасно, но ломает фоновые задачи; I3 противоречит v2-модели. | Явно: «background permission → карточка с меткой background» или «refuse + notice», выбрать осознанно. |
| S2 | `03` §4, D33 | Обоснование «ACP message IDs optional» устарело наполовину: в v1 стабилизированы (опционально), в v2 обязательны; Codex тоже имеет id. | Оставить правило «replay не мержить», но обновить rationale и EV-01. |
| S3 | `03` §2, `05` | Нет ничего про **session config options** (`session/set_config_option`, `config_option_update`) — это теперь основной канал режимов/моделей/permission mode в ACP. | Добавить в контракт адаптера и в pinning. |
| S4 | `06` §3 «Inherited capabilities» | Сбрасывается `SSH_AUTH_SOCK`, но не упомянуты **ControlMaster-сокеты** (`~/.ssh/cm-*`/`ControlPath`): агент того же UID может ходить на удалённые хосты через уже открытый мастер-канал пользователя. Также Keychain-доступные токены `gh`/`gcloud`. | Добавить в S7: доступ к `ControlPath`-сокетам и CLI-токенам; в границу (c)/(e). |
| S5 | `06` §3, `02` §3 | TCC-атрибуция: агенты — «responsible children» Shardrail, значит наследуют его TCC, **включая право модифицировать собственный бандл Shardrail** (App Management разрешает приложению менять себя). Граница (e) «no write to app bundle» на macOS может не выполняться даже при песочнице провайдера, если песочница не закрывает `/Applications/Shardrail.app`. | Явно в S7; исследовать disclaim responsibility при spawn (приватный API — только как исследование) или запуск провайдеров через отдельный helper-бандл. Inferred, требует эксперимента. |
| S6 | `04` §6 purge | Полный `VACUUM` на каждый purge блокирует единственного writer на время перезаписи всей БД и в WAL-режиме раздувает WAL до размера БД (противоречит алярму B8 «WAL ≤ 64 MiB»). На APFS/SSD `secure_delete` не гарантирует физическое стирание. | Crypto-shredding: контент сессии шифровать per-session ключом (Keychain на macOS), purge = удалить ключ + ленивый VACUUM. Заодно решает U12. |
| S7 | `03` §3 D24 | Классы `tested/untested-newer` по точной версии нереалистичны: `claude-agent-acp` вышел 0.24→0.93 за ~7 месяцев (иногда несколько релизов в день), Codex и Grok тоже часто. Пользователь будет почти всегда в strict-режиме. | Классифицировать по **диапазону протокола/схемы** (ACP protocolVersion + capability set; Codex schema hash), а не по semver бинаря. |
| S8 | `09`, `13` R28 | Supply chain почти не покрыт: Node-адаптер Claude ставится пользователем из npm без пина; Hermes — git install; свои зависимости — нет `cargo-deny/cargo-vet`, lockfile-политики, SBOM, подписи артефактов Linux (только checksums), управления ключом Tauri updater. | Добавить раздел в 06/09: lockfiles + `cargo-deny` + `npm audit signatures`, подписанные релизы (minisign/sigstore), пин версий адаптеров с проверкой integrity, хранение updater-ключа офлайн. |
| S9 | `04` §8 handoff | Bundle из вывода агента A → первый prompt агента B = **межагентная prompt-injection**. Есть redaction секретов, нет маркировки/квотирования недоверенного текста. | В bundle разделять «написано человеком» и «цитата из сессии A» с явными разделителями; превью подсвечивает инструкции в цитатах. |
| S10 | `02` §6, `06` §4 | В Phase 1a WebView получает токен от shell; любой XSS в WebView = `full` scope. CSP описан, но не упомянут **Tauri isolation pattern** и capabilities/permissions Tauri 2 (минимальный набор команд IPC). | В D16/D22: включить isolation pattern, запретить все core-плагины кроме нужных, команды IPC — allowlist, отдельный WebView для рендера недоверенного Markdown (или sandboxed iframe). |
| S11 | `06` §4 | `full` scope по code-signing identity → P1a требует Developer ID-подписанных сборок для проверки гейта; dev-сборки всегда `unprotected`. Гейт F22/D20 в P1a не проверяем без релизной инфраструктуры. | Перенести code-signing admission в P1 close/P2; в P1a — только peer UID + токен. |
| S12 | `11` §1 | Граф модулей неполон: trust state (M11) нужен M09 (approvals) и M05/M08 (settingSources при спавне); staleness approvals (M09) читает HEAD/diff hash из M11 — рёбер нет. | Добавить рёбра или порт `WorkspaceState`. |
| S13 | `05` §6 | `deny_after` 180 s и hold-max 2 h: для Hermes предел 300 s, для Codex лимит неизвестен (S1). Hold > лимита провайдера невозможен — нужна автоматическая отсечка «min(policy, known provider limit)», а не ручной ввод значения пользователем. | Читать лимит из протокола, где возможно; иначе показывать «провайдер может отклонить раньше». |
| S14 | `14` §3 | «Claude Code: `claude` not on PATH» — устарело: сейчас `/opt/homebrew/bin/claude` 2.1.292. Codex 0.160.0, Grok 1.0.52 — совпадают. | Обновить. |
| S15 | `02` §1 diagram | «ACP client: Grok, Hermes, Claude via ACP» предрешает S4, тогда как 03 §1 оставляет выбор SDK. | Подпись «Claude via ACP *or* SDK (S4)». |
| S16 | `06` §1 | MCP-серверы, настроенные пользователем, обычно работают **вне** песочницы провайдера (Codex/Claude запускают их как отдельные процессы). Вызов MCP-инструмента с побочным эффектом фактически boundary-breaking, но в классификаторе D19 не учтён. | Классифицировать MCP tool calls с side effects как минимум `widening`; в S7 проверить, наследуют ли MCP-процессы песочницу. |
| S17 | `07` B16 | Git status ≤ 1 s на 100k файлов **при отключённом fsmonitor** (D23 его запрещает) — холодный stat-walk на APFS легко 2–5 s. | Сделать B16 «warm cache», холодный — отдельной метрикой; или опираться на diff от провайдера (C4). |
| S18 | `07` B11 | ≤ 1 wakeup/s на всё дерево WebKit в простое — WebKit-процессы (GPU/Networking) сами по себе могут превышать. Неизмеримо до S5. | Оставить как цель S5, не как гейт P1. |

---

## 4. Fact-check внешних утверждений

| Утверждение плана | Где | Статус | Источник (доступ 2026-10-18) |
|---|---|---|---|
| ACP v1: `session/load` реплеит всю историю; `session/resume` без реплея, capability `sessionCapabilities.resume` | 03 §4, EV-01 | **Верно** для v1; `session/resume` теперь **стабилизирован**. В v2 `session/load` удалён (→ `session/resume` + `replayFrom`) | agentclientprotocol.com/protocol/session-setup; /protocol/v2/migration.md |
| ACP v1 методы: …`session/set_mode`, client `fs/*`, `terminal/*` | EV-02 | **Неполно/устаревает**: в v1 добавлены стабильные `session/list`, `session/close`, `session/delete`, `logout`, elicitation, `$/cancel_request`, config options; в v2 `set_mode`, `fs/*`, `terminal/*` удалены | /protocol/overview; /llms.txt (announcements); v2 migration |
| ACP message IDs опциональны → дедуп невозможен | 03 §4, D33 | **Частично устарело**: стабилизированы (опц.) в v1, **обязательны** в v2; но спецификация v2 всё равно советует не мержить replay по id — вывод D33 верен | announcements/message-id-stabilized; v2 migration «Resuming» |
| Permission outcomes `cancelled`/`selected`; kinds `allow_once/allow_always/reject_once/reject_always`; reject не обязателен | EV-11 | **Верно**. В v2 добавлены `title` (обязат.), `subject` (`tool_call`/`command`/отсутствует); неизвестный outcome агент не должен считать approval | /protocol/tool-calls; v2 migration «Permission requests» |
| ACP v1 — единственная версия | 03 §1 | **Устарело**: v2 в draft, рекомендуется dual-stack; Rust/TS SDK 1.x с роутером v1/v2 | announcements/acp-v2-draft; v2 migration |
| Rust-крейт `agent-client-protocol` официальный | 02 §8 | **Верно**; Rust и TS SDK достигли 1.0 | announcements/sdk-1-0-releases |
| `codex app-server` stdio; WebSocket experimental; `thread/shellCommand` вне песочницы; `item/completed` авторитетен; `clientInfo.name` → compliance logs | 03, EV-03 | **Верно** | learn.chatgpt.com/docs/app-server |
| Codex: background terminals с `osPid` и `thread/backgroundTerminals/terminate` — «Verified» | 03 §5, 05 §1 | **Неточно**: методы **experimental** (нужен `experimentalApi`, что запрещено D24), `osPid` может быть `null` | learn.chatgpt.com/docs/app-server («Clean background terminals») |
| Codex approvals: `accept/acceptForSession/decline/cancel`, `acceptWithExecpolicyAmendment`; permission grants с `scope:"session"`; elicitation; user input | 03 §2, §5 | **Верно**, но **пропущено**: `approvalsReviewer` (может уводить approvals на авто-ревьюер), `networkApprovalContext` (группировка), `tool/requestUserInput.autoResolutionMs`, app-tool approvals через `tool/requestUserInput` | learn.chatgpt.com/docs/app-server; github openai/codex `app-server-protocol/src/protocol/v2/turn.rs` |
| Codex app-server — «documented as the way to embed Codex in a product» | 03 §1 | **Верно с оговоркой**: «app-server command … experimental and not supported for production workloads»; «App-server authentication has never been permitted for commercial or hosted services» | learn.chatgpt.com/docs/app-server |
| Claude: третьи стороны не могут предлагать claude.ai login; API key; Commercial Terms; брендинг «Claude Agent», не «Claude Code» | 03 §6, EV-04 | **Верно**. Дополнение: legal-страница **разрешает** конечному пользователю входить своей подпиской в **немодифицированный** Claude Code, даже если платформа его хостит; запрещено собирать/посредничать токены | code.claude.com/docs/en/agent-sdk/overview; /legal-and-compliance |
| Claude SDK: bare `-p` без permission host denies; `canUseTool` | 03 §5 | **Верно, но неполно**: allow-правила из settings молча обходят `canUseTool`; без явного `permissionMode` сессия может стартовать в `auto` (TS SDK ≥ 0.3.286); есть `permissionPrompts:'none'`, `dontAsk` | code.claude.com/docs/en/agent-sdk/permissions |
| `@agentclientprotocol/claude-agent-acp`, Apache-2.0, организация ACP, не Anthropic | 03 §1, EV-10 | **Верно** (переименован из `@zed-industries/claude-agent-acp` в 0.24.0; текущая 0.93.0 от 2026-10-16; автор Zed Industries; зависит от `@anthropic-ai/claude-agent-sdk`) | npmjs.com/package/@agentclientprotocol/claude-agent-acp; CHANGELOG |
| `grok agent stdio` — ACP entry point; профили `off/workspace/read-only/strict/devbox`; network deny no-op на macOS | 03, EV-09 | **Верно**. **Пропущено**: дефолт sandbox `off`; все профили пишут `~/.grok/`; проектный `.grok/sandbox.toml`; **fail-open** если песочницу не удалось применить; `--always-approve`, `_meta.yoloMode`, `systemPromptOverride`; Grok объявляет `loadSession: true` (resume — не заявлен) | github.com/xai-org/grok-build (xai-grok-shell README); docs.x.ai/build/cli |
| Hermes: `hermes acp`; allow once/session/always/deny; timeout 300 s → deny; `allow_always` пишет постоянный allowlist | 03 §5–§6, EV-06 | **Верно**. **Пропущено**: в ACP маршрутизируются только «dangerous terminal commands» (правки файлов без approval) | hermes-agent.nousresearch.com/docs/user-guide/features/acp; /developer-guide/acp-internals |
| Tauri 2.12.3 (2026-10-10) — текущий stable | EV-08 | **Верно** | v2.tauri.app/release |
| WKWebView не может открыть Unix socket; нужен relay или loopback | 02 §6 | **Верно** (Inferred, общеизвестно); не упомянуто, что Tauri IPC (commands/Channels) — естественный relay без открытия TCP | — |
| macOS: helper отслеживает потомков (best effort) | 06 §8 | **Механизм не назван**; `NOTE_TRACK` → `ENOTSUP` с 10.5; coalitions/responsibility требуют entitlement; рабочая техника — `NOTE_FORK` + `p_puniqueid` + inherited sentinel-fd | github.com/oven-sh/bun commit fcd764b, PR #33965 |
| SQLite: `secure_delete` до delete; `incremental_vacuum` no-op без `auto_vacuum=incremental` | 04 §6, EV-20 | **Верно**; но стоимость полного VACUUM на purge не оценена (S6) | sqlite.org/pragma.html |
| `claude` не на PATH | 14 §3 | **Устарело**: 2.1.292 в `/opt/homebrew/bin` | локальная проверка (read-only) |

---

## 5. Реалистичность бюджетов (07)

- **B1 (40 MB демон idle)** — реалистично с запасом для Rust/tokio/SQLite (обычно 10–25 MB). Ок.
- **B3 (180 MB app + все WebKit-процессы)** — достижимо, но впритык; WebContent + GPU + Networking на Apple Silicon легко дают 120–250 MB physical footprint. Правильно, что решение отложено на S5. Важно мерить одинаковым инструментом (`footprint -p` по всем PID), иначе цифры несопоставимы.
- **B4 (p95 ≤ 50 ms событие→кадр)** — реалистично при коалесинге по кадру.
- **B7, B10, B13, B14** — реалистично; B14 (100 ms) тривиален локально.
- **B8 (72 h soak)**, **B11 (≤1 wakeup/s по всему дереву WebKit)**, **B12** — дорого мерить соло; B11 может быть недостижим не по вине кода.
- **B15/B16** — B16 при запрете fsmonitor нереалистичен на холодную (S17).
**Вывод:** гейтить P1a только B4 и B7 (как и написано), B1/B3 — наблюдать; остальное перевести в «цели» до появления реальных пользователей. Нагрузку на измерения для соло-разработчика план недооценивает: «No budget closes without a stored trace» × 16 бюджетов — это отдельный проект.

---

## 6. Предлагаемые сокращения и упрощения для соло-разработчика

1. **P1a′ «Первый полезный срез»** (вместо текущего P1a):
   - демон + SQLite-журнал (без purge/salvage, только WAL + epoch);
   - **один** ACP-провайдер (Grok или Codex через app-server — см. ниже), dual-stack v1/v2 в адаптере;
   - approvals только `once`/`deny`, journal-before-send (C1), deny по таймауту;
   - diff/файлы — **только из протокола провайдера** (ACP diff content, Codex `turn/diff/updated`/`fileChange`), без собственного Git → safe-Git (S8, M11 worker, B16) уходит из P1a целиком;
   - UI: inbox + одна сессия в Tauri, без menu bar/keychain/notifications;
   - host protection: постоянный баннер «approvals not protected», без claim-логики, без code-signing admission.
2. **Отложить до P1 close/P2:** write lease (пока предупреждение), host protection state machine + S7, containment helper на macOS (пока process group + snapshot дерева), version-skew, orphan-daemon, reconfirmation/staleness, handoff MVP.
3. **Спайки:** вместо S1–S9 — **S2 (Grok)**, **S1-lite (Codex: thread/turn/approval/interrupt + `approvalsReviewer`)**, **S5 (Tauri footprint + transport)**, **S9-lite (macOS: sentinel-fd техника)**. S3/S4/S6/S7/S8 — после P1a′.
4. **Провайдеры в P1:** Grok + Codex. Claude — вне P1 до ответа на U9 (план уже допускает). Hermes — вне P1: нет песочницы, approvals только для shell → для Shardrail-гарантий наименее полезен.
5. **Убрать/упростить:** delivery-таблицу с тремя `resolved_*` состояниями заменить на `unknown → user resolves (resend|discard)`; бейдж-прецеденцию в 05 §7 сократить до 5 уровней; mobile, browser listener, multi-host federation, external history (F15) — вычеркнуть из v1 scope до реального использования.
6. **Процесс плана:** правило «change in place + Revised because» хорошее; добавить к каждому документу дату последней сверки с провайдерами — протоколы меняются еженедельно.

---

## 7. Рекомендации по вопросам владельца

**U13 — Rust или Go (и почему не C#).** План сравнивает только Rust и Go, хотя владелец — опытный C#/Unity-разработчик. Варианты:
- *Rust* — плюсы реальные: официальный ACP SDK 1.x с v1/v2-роутером, open-source типы Codex (`codex-rs/app-server-protocol`), Tauri shell всё равно на Rust, нет GC. Минус — кривая обучения и скорость итерации соло.
- *C# / .NET 10 NativeAOT* — не рассмотрен, а зря: одиночный нативный бинарь, idle ~15–30 MB (укладывается в B1), сильная асинхронность, System.Text.Json source-gen, есть community ACP-библиотеки (`acp-csharp`, `Acp.Net`), типы Codex генерируются из JSON Schema. Минус — нет официального ACP SDK, discriminated unions слабее, чем enum в Rust.
- *Go* — ни одного преимущества перед двумя выше для этого владельца.
**Рекомендация:** если Daliys уже уверенно пишет на Rust — Rust. Если нет — **C# NativeAOT для демона + тонкий Rust в Tauri shell** (шелл не содержит логики по D2). Решить по итогам 2-дневного прототипа «ACP-клиент к `grok agent stdio` + SQLite-журнал» на обоих языках. Schema-first контракты (D27) делают выбор обратимым.

**U9 — Claude.** Различить два пути: (a) Agent SDK / `claude-agent-acp` → по документации SDK только API-key, пока Anthropic явно не одобрит иное; (b) запуск **немодифицированного** `claude`, в который пользователь сам вошёл через flow Anthropic → legal-страница это прямо допускает, но прямого официального протокола с двусторонними approvals, кроме SDK/headless, у CLI нет. **Рекомендация:** для личного/внутреннего использования Fork Horizon — путь (a) с API-ключом пользователя; для коммерции — письменный запрос в Anthropic sales до P1d; Claude держать вне критического пути (как уже в плане). Аналогичный вопрос завести для Codex (U25, C8).

**U8 — SolidJS или React.** При приоритете WCAG 2.2 AA и соло-разработке — **React + React Aria (Adobe)**: зрелые доступные примитивы (listbox, grid, dialog, tree), которых в экосистеме Solid меньше. Производительность стрима определяется коалесингом, виртуализацией и инкрементальным Markdown (07 §4), а не фреймворком; React 19 с этим справляется. Solid выбирать только если S5 покажет, что React не укладывается в B4/B5 — маловероятно.

**U3/U14 — лицензия и видимость.** Пока продукт не существует: **репозиторий приватный, «all rights reserved»**, как и предлагал исходный черновик. Открывать имеет смысл одновременно с первым релизом; разумные кандидаты — source-available (FSL/BUSL с конверсией в Apache-2.0 через 2–4 года) если важна коммерция, или Apache-2.0 если важнее доверие сообщества (инструмент, который управляет approvals, выигрывает от аудируемости кода). Заранее: CLA/DCO не нужен, пока нет внешних контрибьюторов; `NOTICE` и проверка лицензий зависимостей — с первого коммита кода.

**U21 — порог для заявления «protected approvals».** С учётом C3/C7 и S5 почти наверняка ни один провайдер на macOS в дефолтном профиле не пройдёт всю границу D20. **Рекомендация:** не делать «protected» маркетинговым свойством вообще в v1. Вместо бинарного статуса показывать per-session перечень того, что **проверено** («агент не может писать в данные Shardrail: да/нет; в свой конфиг approvals: да/нет; выход в сеть: да/нет»), и оставить правило: claim «protected» только если **все** запущенные на хосте процессы проходят S7-проверку **и** применение песочницы подтверждено в рантайме (не по флагу). Предрегистрированное правило из 06 §3 сохранить — оно правильное.

---

## 8. Что в плане хорошо и стоит сохранить

Таймаут/дисконнект никогда не = allow; отказ от неизвестных серверных запросов; запрет `thread/shellCommand`; разделение двух осей (sandbox vs approval policy); safe-Git правило «никаких программ из git-конфига»; эфемерные vs durable события с append-only чекпоинтами; `command_id` + preconditions; клиентская федерация хостов без репликации; SSH stdio-bridge вместо проброса портов; честные метки уверенности и реестр «Revised because».


---

## Resolution status (2026-10-18)

*Added after the findings were applied in place (in English, like the rest of the plan). "Addressed" = the plan now contains the fix; "partial" = fix recorded but depends on a spike or owner answer; "deferred" = intentionally not applied yet. Owner questions remained open at the time; by 2026-10-19 all were answered (13 §1a, 17) except U5/U7, deferred.*

| Finding | Status | Where |
| --- | --- | --- |
| C1 approval write-ahead | addressed | 05 §4 (`responding`, D34), 04 §4, 01 F23, 12 D34, 10 T1.11 |
| C2 ACP v2, Codex `approvalsReviewer`, experimental methods, D24 contradiction | addressed | 03 §1–§5 (D35, D36, D37), 05 ACK rules, 12 D24/D35–D37, 14 EV-22–EV-24; behaviour verified in S1-lite/S2 |
| C3 Grok sandbox / P1a provider | addressed | 03 §6 (R34), 06 §3/§3a, 10 P1a (D40), 13 R34 and tensions |
| C4 P1a too large | addressed | 10 §1 (slim P1a, new P1e), §2 core spikes, §3 epics E1.3/E1.7/E1.9/E1.12/E1.18, §5 critical path; 12 D40 |
| C5 daemon upgrade kills turns | addressed | 02 §3, 05 §1 (D39, `T_drain`), 04 §7, 01 F25, 10 T1.14 |
| C6 agent-authored approval text | addressed | 06 §5 (D38), 08 approval card, 10 T1.10 |
| C7 provider-internal bypass paths | addressed (verification pending S7) | 06 §3a table, 03 §5–§6, 08 per-session indicator, 13 R33–R36, R42 |
| C8 Codex terms | **resolved by owner (2026-10-19):** internal use (D45), user's own login; reopen trigger R46 | 03 §6, 09 §2, 12 D45, 13 §1a/R43/R46 |
| C9 macOS containment mechanism | partial (S9) | 06 §8 (D43), 11 M05, 10 S9/S9-lite, 14 EV-28 |
| C10 queue, close/purge, journal corruption/disk full | addressed | 05 §5a, §9, §10 (D41, D42), 04 §6–§7, 01 F7/F17/F24 |
| S1 background approval in `idle` | addressed | 05 §3 (reference fixed, background note) |
| S2 D33 rationale | addressed | 03 §4, 12 D33, 14 EV-01 note |
| S3 session config options | addressed | 03 §2, 05 route exception, 11 M06 |
| S4 ControlMaster / CLI tokens | addressed (verification S7) | 06 §3 inherited capabilities, 10 S7, 13 R39 |
| S5 TCC bundle write | partial (Inferred, S7) | 06 §3, 10 S7, 13 R40 |
| S6 purge VACUUM cost | partial — owner 2026-10-19: no encryption in v1, crypto-shredding deferred (U12) | 04 §6 (costs, crypto-shred recommendation), 13 U12 |
| S7 version classes | addressed | 03 §3 (protocol fingerprint, D36) |
| S8 supply chain | addressed; release-signing parts (SBOM, signed artifacts, updater key) deferred until other users (D46) | 06 §12, 09 §2/§5, 13 R41 |
| S9 handoff injection | addressed | 04 §8, 13 R38 |
| S10 Tauri isolation/capabilities | addressed | 06 §5 WebView hardening, 12 D22 |
| S11 code-signing admission in P1a | addressed; P1e verifies Daliys's own local/self-signed identity (D46) | 06 §4 (moved to P1e), 10 P1a/P1e |
| S12 module edges | addressed | 11 (`WorkspaceState` port, M11 → M09/M05) |
| S13 expiry vs provider limit | addressed | 05 §6 `deny_after` |
| S14 stale `claude` fact | addressed | 14 §3 |
| S15 diagram label | addressed | 02 §1 |
| S16 MCP outside sandbox | addressed (verification S7) | 03 §2 option classes, 06 §3a, 13 R42 |
| S17 B16 cold cache | addressed | 07 B16 |
| S18 B11 | addressed | 07 B11 (target, not gate) |
| §4 fact-check | addressed | 14 EV-22–EV-31 and annotations on EV-01/02/03/06/10/21 |
| §5 budgets / measurement load | addressed | 07 gates (P1a: B4, B7; B1/B3 observed), measurement note |
| §6.1–6.3 P1a′, deferrals, spikes | addressed | 10 §1–§3 |
| §6.4 providers in P1 | addressed — owner 2026-10-19: Hermes not in Phase 1 (D48); Claude terms resolved by D29/D45, placement per S4 | 10 P1c/P1d, 12 D48, 13 §1a |
| §6.5 delivery-table and badge simplification; cutting mobile/browser/federation/F15 from v1 | deferred | needs owner scope decision; 05 §5/§7 and 01 scope unchanged |
| §6.6 doc freshness | addressed | 13 tensions, 00 next steps |
| §7 U13 recommendation | **resolved by owner (2026-10-18): Rust**; C#/.NET and Go rejected, no dual prototype | 12 D1 (owner-approved), 13 §1a, 02 §8 |
| §7 U8 recommendation | **resolved by owner (2026-10-18): React 19 + TS strict + Vite SPA + React Aria + TanStack Virtual**; Svelte 5 fallback if S5 fails B3/B4/B5 | 12 D3, 13 §1a, 02 §8, 08 §6, 16 |
| §7 U9 recommendation (and U25) | **resolved by owner (2026-10-19):** path (a), Agent SDK / adapter with the user's own API key; internal-use scope, no requests to Anthropic/OpenAI now | 12 D29/D45, 13 §1a, 03 §6, 09 §2 |
| §7 U3/U14 recommendation | **resolved by owner (2026-10-19):** private, proprietary/all rights reserved for now; open-licensing options parked until other users (R47) | 12 D46, 13 §1a, 09 §2/§5 |
| §7 U21 recommendation | **resolved by owner (2026-10-19):** no "protected" status in v1; per-session verification checklist; S7 gates any future status (separate decision) | 12 D47, 06 §3b, 13 §1a |
| §7 U12 / U22 recommendations | **resolved by owner (2026-10-19):** U12 no encryption in v1 (crypto-shredding deferred); U22 never auto-terminate, mark desynced, inbox | 13 §1a, 04 §6–§7, 05 §3, 17 |
| §6 core spikes S1-lite, S2, S5, S9-lite | **authorized by owner 2026-10-19** | 10 §2, 00 |
