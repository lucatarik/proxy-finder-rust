# Proxy Finder — guida

Proxy Finder scarica liste di proxy pubblici da GitHub, le unisce senza duplicati,
le prova in due passaggi (http e https/CONNECT) e tiene l'elenco dei proxy funzionanti con
latenza, IP di uscita e nazione. In più:

- **23 sorgenti** incluse (liste pubbliche su GitHub), più quelle che
  aggiungi tu; puoi abilitarle, disabilitarle, rimuoverle e ripristinare le predefinite;
- un **rating per sorgente** che sale con ogni proxy buono trovato in quella lista (tanto più
  quanto il proxy è buono) e scende con ogni proxy morto;
- un **relay locale** su `127.0.0.1:8899`: "Connetti" lo collega al proxy scelto, e qualsiasi
  programma configurato su quel proxy HTTP esce dall'IP del proxy pubblico.

## Build e avvio

```bash
./proxy-finder.sh            # GUI (compila in release la prima volta)
./proxy-finder.sh help       # modalità a riga di comando: vedi CLI.md
cargo run --release          # GUI
cargo test                   # test offline (parser, unione, rating, tester, store, relay)
cargo run --release --example smoke -- 128 monosans proxifly   # prova reale in rete
```

Richiede Rust 1.82+. Su Linux servono le librerie di X11 o Wayland (eframe con backend glow).

## Schede

### Proxy
I proxy verificati: paese (bandiera e codice ISO a 3 lettere), `ip:porta`, tipo, latenza colorata (verde < 300 ms, gialla < 1000 ms,
rossa oltre), IP di uscita, score, sorgenti che lo elencano, ultima verifica.

- click sull'indirizzo: il proxy viene **riverificato** e, solo se risponde, diventa quello del
  relay; se non risponde viene marcato **non risponde**;
- **Connetti al più veloce** (in alto): riverifica i proxy https in ordine di latenza, 8 alla volta,
  e si collega al primo che risponde. I "solo http" non vengono mai scelti in automatico;
- **Ricontrolla** rimette alla prova tutti i buoni; **Elimina non funzionanti (n)**;
- **Copia tutti** e **Esporta** in `.txt` (`proto://ip:porta`, uno per riga) o `.json`.

I proxy che reggono l'http ma non `CONNECT` hanno il badge rosso **SOLO HTTP**: connessi a uno di
questi, l'intestazione mostra una fascia rossa, perché i siti https non si aprono e quelli http
passano in chiaro.

### Scansione
- **Scarica abilitate**: scarica le sorgenti abilitate (6 alla volta, timeout 25 s, massimo 32 MB
  per file), unisce le liste eliminando i duplicati e alternandole una voce per sorgente, senza
  limite di voci. Un proxy presente in più liste resta una voce sola che ricorda **tutte** le sue
  sorgenti. I proxy già provati (storico) o già tra i buoni non rientrano;
- **Test**: protocollo, paese (`ITA`, `USA, DEU`…; vanno bene anche i codici a 2 lettere), quanti (multipli di 8, almeno 8, nessun massimo; **tutti** prende l'intera lista filtrata), quanti in
  parallelo, timeout (rapidi 5/8 s, normali 10/15 s, pazienti 15/25 s), riprova dopo un timeout,
  scarta chi non regge https (vale per ogni protocollo, socks compresi: conta l'esito della prova TLS, non il tipo di proxy), ordine **prima i più promettenti** (rating della sorgente × metadati
  dichiarati) o casuale;
- **Annulla** ferma subito le prove in corso: quelle interrotte non contano (né storico né rating);
- registro live degli ultimi 200 esiti.

### Sorgenti
Tabella con abilitazione, nome (link al repository), rating 0–100 con barra, provati / buoni /
morti con percentuale di vivi, ultimo download (voci o errore), numero di endpoint (passa sopra
per vedere URL e formato). Azioni: **Aggiungi** (nome, URL, formato, protocollo), **rimuovi** (con
conferma, anche le predefinite), **azzera** rating, **Ripristina predefinite** (rimette quelle
incluse e conserva rating e sorgenti personali).

Formati supportati per le sorgenti personali:

| Formato | Esempio |
|---|---|
| Auto | riga per riga: `proto://ip:porta` oppure `ip:porta` |
| ip:porta | `1.2.3.4:8080` (protocollo scelto nel modulo) |
| ip:porta:paese | `1.2.3.4:8080:Germany` |
| proto://ip:porta | `socks5://1.2.3.4:1080` |
| JSON monosans / proxifly / vakhov | i formati di quei repository |

Righe vuote, commenti `#`, messaggi tipo `error code: 502`, IP o porte non validi vengono scartati.

### Impostazioni
Porta del relay, **Verifica IP** (IP reale e IP visto attraverso il proxy attivo), cartella dei dati.

## Icona nel vassoio

`proxy-finder tray` mette un'icona nel vassoio di sistema con **Collega**, **Cambia proxy** e
**Scollega**: trova e verifica un proxy come `--auto`, avvia il relay e imposta il proxy di
sistema GNOME (lo segue Firefox con "Usa le impostazioni proxy del sistema"). Dettagli in
`CLI.md`.

## Rating delle sorgenti

Ogni proxy buono ha uno **score** in [0, 1]:

```
score = clamp(1 − latenza_ms / 5000, 0.05, 1) × (1 se regge https, 0.5 se solo http)
```

Il rating di una sorgente è una media degli score dei proxy che ha fornito, con un prior:

```
rating = (somma degli score dei buoni + prior × 5) / (proxy provati + 5)
```

- un proxy **buono** aggiunge il suo score al numeratore e 1 al denominatore: uno veloce con
  https alza il rating più di uno lento o solo http;
- un proxy **morto** aggiunge 0 al numeratore e 1 al denominatore: il rating scende;
- il **prior** (0,50 per le sorgenti di qualità alta, 0,35 per le altre, 0,30 per quelle
  aggiunte a mano) vale come 5 prove fittizie: poche prove non spostano troppo il valore, e
  una lista enorme non va a meno infinito perché il rating resta sempre tra 0 e 1;
- un proxy elencato da più liste accredita l'esito a **tutte**;
- solo la **prima** prova di un candidato conta: ricontrolli e connessioni non toccano i rating.

Esempio: sorgente con prior 0,35, si provano 30 suoi proxy; 6 sono buoni (4 https a 400 ms,
score 0,92; 2 solo http a 1500 ms, score 0,35), 24 sono morti.
Rating = (4×0,92 + 2×0,35 + 0,35×5) / (30 + 5) = (3,68 + 0,70 + 1,75) / 35 = **0,175**, cioè
17,5 su 100.

Il rating decide l'ordine dei test ("prima i più promettenti"): priorità = rating migliore tra le
sorgenti del proxy × (0,5 + 0,5 × metadati), dove i metadati sono la latenza dichiarata o
l'uptime. L'ordinamento è a fasce di 0,05 e dentro ogni fascia resta l'alternanza tra
sorgenti, così una lista enorme con metadati appena migliori non monopolizza la scansione.

## Come vengono verificati

Per ogni candidato si crea un client HTTP che usa quel proxy (`http`, `socks4`, `socks5`):

1. **passaggio A**: `GET http://<eco>` su uno dei 25 worker Cloudflare `st-echo-NN` o su
   ipify / icanhazip / amazonaws (a rotazione). Passa chi risponde 200 con un IP **diverso** dal
   tuo; dopo un errore di rete c'è un secondo tentativo su un'altra eco. Se il proxy ha risposto
   ma rifiuta l'http in chiaro (stato HTTP, risposta senza IP, tunnel o SOCKS rifiutato verso la
   porta 80) il passaggio A si ripete subito in `https`: molti proxy, socks compresi, accettano
   solo CONNECT o solo la 443. Un proxy irraggiungibile (connessione rifiutata), in timeout o
   trasparente è scartato subito;
2. **passaggio B**: `GET https://<eco>` su un'eco diversa, quindi tunnel CONNECT e sessione TLS
   vera. Chi lo supera è un proxy completo; chi no, dopo una seconda conferma in http, è
   **solo http** (o viene scartato con l'opzione apposita). Chi ha superato il passaggio A solo
   in https e fallisce il B è scartato come instabile.

Latenza = media dei due passaggi. La nazione è quella calcolata dal worker Cloudflare sull'IP di
uscita, altrimenti quella dichiarata dalla lista. L'IP reale si chiede in diretta prima del test
(ipify, poi `1.1.1.1/cdn-cgi/trace`): senza, il test non parte. Le prove girano in un pool a N
corsie: appena una finisce, la corsia prende il candidato successivo.

## Relay

Il relay ascolta solo su `127.0.0.1` e accetta richieste da proxy HTTP:

- `CONNECT host:porta` (https): tunnel attraverso il proxy scelto (CONNECT verso un proxy http,
  handshake SOCKS4/SOCKS5 verso un proxy socks);
- `GET http://…` in forma assoluta: inoltrata al proxy http così com'è, oppure via SOCKS con la
  request-line riscritta in forma origine.

Senza proxy connesso risponde `503`: niente esce in chiaro dal relay. Per usarlo:

```bash
chromium --proxy-server="http://127.0.0.1:8899"
curl -x http://127.0.0.1:8899 https://api.ipify.org
```

## Sorgenti incluse

| id | Repository | Formato | Note |
|---|---|---|---|
| monosans | monosans/proxy-list | JSON | latenza, paese |
| proxifly | proxifly/free-proxy-list (solo socks5) | proto://ip:porta | |
| vakhov | vakhov/fresh-proxy-list | JSON | delay, uptime |
| hideip | zloi-user/hideip.me | ip:porta:paese | |
| speedx | TheSpeedX/PROXY-List | ip:porta | |
| hookzof | hookzof/socks5_list | ip:porta | solo SOCKS5 |
| roosterkid | roosterkid/openproxylist | ip:porta | |
| sunny9577 | sunny9577/proxy-scraper | ip:porta | |
| zaeem | Zaeem20/FREE_PROXIES_LIST | ip:porta | |
| proxyscrape | API ProxyScrape v4 | proto://ip:porta | non è GitHub |
| jetkai | jetkai/proxy-list | ip:porta | |
| rdavydov | rdavydov/proxy-list | ip:porta | |
| iplocate | iplocate/free-proxy-list | proto://ip:porta | |
| dpangestuw | dpangestuw/Free-Proxy | proto://ip:porta | |
| r00tee | r00tee/Proxy-List | ip:porta | |
| databay | databay-labs/free-proxy-list | ip:porta | |
| vann-dev | Vann-Dev/proxy-list | ip:porta | |
| elliottophellia | elliottophellia/proxylist | ip:porta | liste già verificate |
| clarketm | clarketm/proxy-list | ip:porta | |
| prxchk | prxchk/proxy-list | ip:porta | lista corta |
| anonym0us | Anonym0usWork1221/Free-Proxies | ip:porta | |
| tuanminpay | TuanMinPay/live-proxy | ip:porta | liste enormi (socks5 ~80.000) |
| ercindedeoglu | ErcinDedeoglu/proxies | ip:porta | liste enormi; `https.txt` dichiarato https |

Tutte verificate (HTTP 200) il 2026-10-19. Esclusi: casals-ar (9 MB), MuRongPIG (http e socks5
identici), SevenworksDev (IP anycast Cloudflare), mmpx12 e ALIILAPRO (rate limit), KangProxy e
gfpcom (404).

## Dati

In `~/.config/proxy-finder/` (o in `$PROXY_FINDER_HOME`): `sources.json`, `proxies.json`,
`candidates.json` (tuple compatte `[proto, ip, porta, paese, latenza, uptime, [sorgenti], https dichiarato]`; si leggono anche quelle a 7 elementi delle versioni precedenti),
`history.json`, `settings.json`. Le scritture sono atomiche; un file corrotto viene copiato in
`<file>.bad` e sostituito dal default.

## Limiti e avvertenze

- I proxy pubblici vedono i domini che visiti e, in http, anche il contenuto; possono modificarlo.
  Non usarli per banche, password o pagamenti.
- Con i proxy SOCKS4 la risoluzione DNS avviene in locale (il protocollo vuole un IPv4).
- I proxy cambiano di continuo: uno verificato ieri può essere morto oggi. Usa **Ricontrolla**.
- I worker `st-echo-NN.lucatarik.workers.dev` condividono il limite gratuito di Cloudflare
  (100.000 richieste al giorno).

## Struttura

```
src/lib.rs        moduli della libreria (senza GUI)
src/model.rs      Protocol, Format, Source, Candidate, GoodProxy
src/parse.rs      parser dei formati
src/sources.rs    sorgenti predefinite, download, unione round-robin
src/rating.rs     score dei proxy, rating delle sorgenti, priorità
src/tester.rs     test a due passaggi, pool, parse_echo
src/store.rs      persistenza JSON, impostazioni
src/relay.rs      proxy HTTP locale verso l'upstream
src/job.rs        job asincroni ed eventi per la UI
src/state.rs      stato persistente e regole sugli esiti, condiviso da GUI e CLI
src/cli.rs        modalità a riga di comando (CLI.md)
src/tray.rs       icona nel vassoio: collega/scollega via `--auto`, proxy di sistema GNOME
src/countries.rs  nomi inglesi dei paesi -> alpha-2, alpha-3, bandiere
src/country_codes.rs, assets/flags.png
                  codici ISO e atlante delle bandiere, generati da tools/make_flags.py
src/main.rs, src/ui/   GUI eframe/egui (Proxy, Scansione, Sorgenti, Impostazioni)
tests/core.rs     test d'integrazione offline con eco e proxy finti
examples/smoke.rs prova reale in rete
proxy-finder.sh   avvio (compila se serve)
```
