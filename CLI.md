# Proxy Finder da riga di comando

Lo stesso programma della GUI, senza finestra: senza argomenti si apre la GUI, con un comando si
lavora dal terminale. CLI e GUI usano **gli stessi dati** (`~/.config/proxy-finder`, oppure
`$PROXY_FINDER_HOME`) e **le stesse regole**: ciò che trovi da una parte lo vedi dall'altra.
Non usare però le due modalità **nello stesso momento** sugli stessi dati: chi salva per ultimo
sovrascrive l'altro.

## Avvio

```bash
./proxy-finder.sh                  # GUI (senza display mostra l'aiuto della CLI)
./proxy-finder.sh help             # elenco dei comandi
./proxy-finder.sh test -n 64       # un comando qualsiasi
```

`proxy-finder.sh` compila in release la prima volta e ogni volta che i sorgenti cambiano, poi
avvia `target/release/proxy-finder` passando gli argomenti. Serve `cargo` (https://rustup.rs).
Il binario si può anche usare direttamente o copiare nel `PATH`:

```bash
cp target/release/proxy-finder ~/.local/bin/
proxy-finder status
```

## Flusso tipico

```bash
proxy-finder download              # scarica e unisce le sorgenti abilitate
proxy-finder test -n 128           # prova i 128 candidati più promettenti
proxy-finder list                  # proxy funzionanti, i più veloci prima
proxy-finder relay                 # relay su 127.0.0.1:8899 collegato al più veloce
```

Oppure tutto in un colpo: `proxy-finder --auto` (vedi `auto`).

In un altro terminale:

```bash
curl -x http://127.0.0.1:8899 https://api.ipify.org      # deve stampare l'IP del proxy
chromium --proxy-server="http://127.0.0.1:8899"
```

## Comandi

### `status`
Riepilogo: cartella dati, candidati non ancora provati (per protocollo), proxy buoni
(funzionanti, con https, che non rispondono), storico dei test, sorgenti abilitate, le tre con
rating migliore, opzioni di test salvate, tabella ASN, rating per rete, porta del relay.

### `sources` — gestione delle sorgenti

| Comando | Effetto |
|---|---|
| `sources` / `sources list` | tabella: abilitata (`x`), id, rating 0–100, provati/buoni/morti, ultimo download, nome |
| `sources enable <id>...` | abilita le sorgenti indicate |
| `sources disable <id>...` | le disabilita |
| `sources enable --all` / `disable --all` | tutte |
| `sources add <nome> <url> [--format F] [--protocol P]` | aggiunge una sorgente personale |
| `sources remove <id>...` | rimuove (anche le predefinite) |
| `sources reset [<id>...]` | azzera il rating (tutte se non indichi id) |
| `sources restore` | rimette le predefinite, conserva rating e sorgenti personali |

Formati per `--format` (default `auto`):

| Formato | Righe attese |
|---|---|
| `auto` | `proto://ip:porta` oppure `ip:porta` |
| `ipPort` | `1.2.3.4:8080` |
| `ipPortCountry` | `1.2.3.4:8080:Germany` |
| `protoUrl` | `socks5://1.2.3.4:1080` |
| `monosans`, `proxifly`, `vakhov` | JSON di quei repository |

`--protocol` (`http`, `socks4`, `socks5`) vale per le righe senza protocollo; di default `http`.
L'id della sorgente nuova si ricava dal nome (`"Mia lista"` → `mia-lista`).

```bash
proxy-finder sources add "Mia lista" https://raw.githubusercontent.com/prxchk/proxy-list/main/http.txt --format ipPort
proxy-finder sources disable --all && proxy-finder sources enable monosans proxifly mia-lista
```

### `download`
Scarica le sorgenti abilitate (6 alla volta), unisce le liste senza duplicati alternando una voce
per sorgente, senza limite di numero. Esclude i proxy già provati (storico) e quelli già buoni. Stampa una
riga per sorgente (voci o errore) e il totale. Sostituisce la lista scaricata precedente.
Se manca o ha più di 7 giorni, scarica anche la tabella IP → ASN per il rating per rete (un errore
lì non ferma il download).

### `test [opzioni]`
Prova i candidati scaricati e aggiorna buoni, storico e rating delle sorgenti. Stampa una riga per
esito (`OK` / `KO`), poi `N provati · M buoni · K aggiunti`.

| Opzione | Valori | Default |
|---|---|---|
| `-n`, `--count N` | quanti provare; arrotondato al multiplo di 8 (almeno 8, nessun massimo) | 64 |
| `-p`, `--protocol P` | `http`, `socks4`, `socks5`, `all` | `all` |
| `-c`, `--country CC` | `ITA` o `ITA,USA` (vanno bene anche `IT`, `IT,US`); `all` | `all` |
| `-j`, `--parallel N` | prove contemporanee, multiplo di 8 (8–512) | 64 |
| `-t`, `--timeout T` | `rapidi` 5/8 s, `normali` 10/15 s, `pazienti` 15/25 s (anche `fast`/`normal`/`patient`) | `rapidi` |
| `--order O` | `priority` (rating della sorgente × metadati) o `random` | `priority` |
| `--retry-timeout` / `--no-retry-timeout` | ripete anche dopo un timeout | no |
| `--drop-http-only` / `--keep-http-only` | scarta chi non regge https, qualunque protocollo (socks compresi): passa in chiaro ma non apre una TLS verso la porta 443 | tiene |
| `--save` | salva queste opzioni come predefinite (anche per la GUI) | — |

Senza `--save` le opzioni valgono solo per questa esecuzione.

```bash
proxy-finder test -n 256 -j 128 -p socks5 -c IT,DE -t normali
proxy-finder test --order random --drop-http-only --save
```

### `list [--all] [--json] [--sort latency|added]`
Proxy buoni: paese (bandiera e codice a 3 lettere), `proto://ip:porta`, latenza, IP di uscita, score, stato (`ok`, `SOLO HTTP`,
`non risp.`), sorgenti. Di default solo quelli funzionanti, i più veloci prima. `--all` include
quelli che non rispondono; `--json` stampa i dati completi.

### `recheck`
Riprova tutti i buoni con i timeout normali. Chi non risponde viene marcato `non risp.`, chi
risponde aggiorna latenza e IP di uscita. **Non** modifica i rating.

### `export [FILE] [--json]`
Proxy funzionanti, uno per riga (`proto://ip:porta`), su stdout o nel file indicato; `--json` per
i dati completi.

```bash
proxy-finder export > proxies.txt
proxy-finder export buoni.json --json
```

### `remove <proto://ip:porta>...` e `prune`
`remove` toglie i proxy indicati dai buoni; `prune` toglie tutti quelli marcati `non risp.`.

### `clear list|history|nets`
`clear list` svuota la lista scaricata. `clear history` svuota lo storico dei test: al prossimo
`download` i proxy già provati possono tornare. `clear nets` azzera il rating per rete.

### `nets [asn|24] [-n N]`
Rating per rete: per ASN (provider) e per /24, le N reti migliori e le N peggiori (default 10)
fra quelle con almeno 20 prove. `x` è il moltiplicatore della priorità di prova: 1 = come la
media, 2 = il doppio, 0,2 = un quinto. Attivo dopo 100 prove con almeno un buono.

```
$ proxy-finder nets asn -n 2
4096 prove, score medio 0.0076 per prova; x = moltiplicatore della priorità (reti con almeno 20 prove)

ASN: 22 reti
     x  PROVATI  BUONI  RETE
  2.91       29      2  AS14061 DIGITALOCEAN-ASN
  2.37       58      6  AS16509 AMAZON-02
     …  (18 reti)
  0.29      124      0  AS13335 CLOUDFLARENET
  0.27      134      0  AS45102 ALIBABA-CN-NET Alibaba US Technology Co., Ltd.
```

### `ip [proto://ip:porta]`
Stampa l'IP reale. Con un proxy, anche l'IP di uscita visto attraverso di lui, e avvisa se
coincide con quello reale.

### `relay [--port N] [proto://ip:porta]`
Apre il relay HTTP su `127.0.0.1` (porta dalle impostazioni, default 8899) e lo collega:

- senza argomenti, al proxy https più veloce tra i buoni: li riverifica in ordine di latenza, 8
  alla volta, e usa il primo che risponde;
- con `proto://ip:porta`, a quel proxy (anche se non è tra i buoni) dopo averlo verificato.

Resta in primo piano finché premi **Ctrl+C**. Senza un proxy collegato il relay non parte.
Il relay accetta `CONNECT` (https) e richieste `http://` assolute; per i proxy SOCKS fa
l'handshake SOCKS4/5. Se il proxy è solo http lo segnala: i siti https non si aprono e quelli
http passano in chiaro.

Se la porta è già tenuta da un altro `relay` o `auto` di proxy-finder da riga di comando, non
apre un secondo relay: verifica il proxy, chiede all'istanza in esecuzione di passare a quello
ed esce (vedi sotto, *Cambiare il proxy di un relay già attivo*).

```bash
proxy-finder relay
proxy-finder relay --port 3128 socks5://1.2.3.4:1080
```

### `auto [opzioni]` (anche `--auto`)
Trova un proxy nuovo e ci collega il relay, senza altri passaggi:

1. prova **128** proxy mai provati, tutti in parallelo, scartando chi non regge https
   (connessioni cifrate: tunnel `CONNECT` o SOCKS verso la porta 443 con TLS vera). Prima i
   più promettenti: i **socks5** e gli http che una sorgente dichiara **https** (file
   `https.txt` di hideip, `HTTPS_RAW.txt` di roosterkid, `"protocol": "https"` di
   monosans, `ssl` di vakhov, righe `https://` nei formati con protocollo); dentro ciascun gruppo
   vale l'ordine scelto (`--order`). Finiti quelli passa agli altri;
2. appena ne ha trovati **10** con https ferma la scansione: le prove ancora in corso finiscono
   annullate e quei proxy restano in lista per la prossima volta;
3. se 128 non bastano prova i 128 successivi; se la lista è vuota scarica prima le sorgenti
   abilitate (una volta sola per esecuzione);
4. **prova di velocità prima di collegarsi**: aspetta un tempo casuale fra 3 e 8 secondi, poi
   riprova da capo tutti i trovati in parallelo (stessa prova di `test`, https compreso) e sceglie
   il **più veloce** di questa seconda prova. Chi risponde resta tra i buoni con la latenza
   aggiornata, chi non risponde più resta tra i buoni come "non risponde"; se non risponde nessuno
   riprende la scansione;
5. con `--speed`, invece di prendere chi risponde prima, scarica 2 MB
   (`speed.cloudflare.com`) attraverso ciascun proxy che ha risposto, 3 alla volta, e sceglie il
   più veloce in download. Il tempo comprende connessione e TLS. Il tetto parte da 15 secondi e,
   appena un proxy finisce i 2 MB, scende al suo tempo: chi lo supera viene fermato e scartato,
   perché sarebbe comunque più lento. Se nessuno finisce entro 15 secondi vince chi ha scaricato
   di più; se nessuno scarica niente, usa quello che risponde prima;
6. avvia il relay verso il proxy verificato, come `relay`.

Esiti, rating delle sorgenti, storico e proxy buoni si salvano come con `test` (la verifica del
punto 4 non tocca i rating, come `recheck`). Se la lista finisce con meno di 10 proxy https, usa
quelli trovati; se non ne ha trovato nessuno, o nessuno risponde alla verifica, esce con errore.

**Output**: senza `-v` stampa su stdout **solo l'IP di uscita** del proxy scelto, una riga; se
più tardi un'altra istanza gli cambia il proxy, stampa il nuovo IP di uscita su una nuova riga.
Su stderr vanno solo gli errori veri. Con `-v` (`--verbose`) stampa tutto: avanzamento, esiti
delle prove, proxy scelto, dettagli del relay.

Opzioni: `--port N` (porta del relay), `--speed`, `-v` e, come per `test`, `-p`, `-c`, `-t`, `--order`, `-q`
(quest'ultimo vale con `-v`). Quanti provare (128), quanti in parallelo (128) e lo scarto dei
"solo http" sono fissi e non cambiano le impostazioni salvate.

L'indicazione "dichiarato https" si legge al download: una lista scaricata con una versione
precedente alla 0.1.7 non ce l'ha (lì contano solo i socks5) finché non la riscarichi con
`proxy-finder download`.

```bash
proxy-finder --auto                 # relay su 127.0.0.1:8899, stampa l'IP di uscita
proxy-finder --auto -v -p socks5 -c ITA,DEU -t normali --port 3128
proxy-finder --auto --speed         # sceglie il più veloce in download, non in latenza
```

Il primo `--auto` resta in esecuzione (tiene il relay), quindi `ip=$(proxy-finder --auto)` non
ritorna: per leggere l'IP lancialo in background con l'output su file. Il secondo, che cambia il
proxy all'istanza già attiva, esce subito e lì `ip=$(proxy-finder --auto)` funziona.

### `tray [--port N]`
Solo Linux: su Windows e macOS il comando esce con un errore. Icona nel vassoio di sistema (StatusNotifierItem: LXQt, KDE, XFCE con il plugin, GNOME con
l'estensione AppIndicator). Il pallino cambia colore: grigio scollegato, giallo ricerca in corso,
verde collegato, rosso errore. Il suggerimento mostra l'IP di uscita. Menu:

- **Collega**: avvia `proxy-finder --auto --port N` in background; quando il relay è attivo
  imposta il proxy di sistema GNOME (`gsettings`, schema `org.gnome.system.proxy`: modalità
  manuale, http e https su `127.0.0.1:N`), ricordando i valori di prima;
- **Cambia proxy**: lancia un secondo `--auto`, che trova e verifica un altro proxy e lo passa al
  relay già attivo (le connessioni aperte finiscono sul vecchio);
- **Scollega**: ferma il relay (come Ctrl+C) e rimette il proxy di sistema com'era;
- **Esci**: scollega ed esce. Anche la chiusura della sessione (SIGTERM) scollega.

Il proxy di sistema GNOME lo seguono Firefox con "Usa le impostazioni proxy del sistema" e i
programmi GTK/GIO. Chromium su LXQt non lo legge: avvialo con
`--proxy-server=http://127.0.0.1:8899`. Se all'avvio il proxy di sistema punta ancora al relay ma
il relay non risponde (chiusura brusca), il vassoio lo rimette su "nessuno".

La porta predefinita è quella delle impostazioni, la stessa che usa la GUI per il suo relay:
con la GUI aperta usa un'altra porta (`tray --port 8898`) o "Collega" darà "porta occupata".

Per averlo all'accesso, in LXQt: *Preferenze → Sessione LXQt → Avvio automatico* con il comando
`proxy-finder tray`.

### Cambiare il proxy di un relay già attivo
Rilanciare `proxy-finder --auto` (o `relay`) con la stessa porta mentre un'altra istanza tiene
aperto il relay cambia il proxy **di quell'istanza**: la nuova esegue la scansione, poi chiede al
relay in esecuzione di usare il proxy trovato, aspetta la conferma (al più 5 s) ed esce. Il
vecchio relay stampa il nuovo IP di uscita (con `-v`: `cambio proxy richiesto da un'altra
istanza: <vecchio> -> <nuovo>`).

- Le connessioni già aperte restano sul proxy precedente finché si chiudono; le nuove usano il
  nuovo.
- Funziona tramite due file nella cartella dati: `relay.json` (porta, pid e proxy dell'istanza
  che tiene il relay, tolto all'uscita con Ctrl+C) e `relay-switch.json` (la richiesta di cambio).
  Il relay controlla ogni secondo solo la data di modifica della richiesta e apre il file
  quando c'è ed è cambiato.
- Se la porta è occupata da altro (per esempio la GUI, che tiene il suo relay) il comando esce
  con errore.

## Opzioni generali

- `-q`, `--quiet`: solo i proxy buoni e i riepiloghi. L'avanzamento esce su stderr ogni 5 secondi.
- **Ctrl+C** durante `download`, `test`, `recheck` o la verifica di `relay` annulla il job
  subito. Le prove interrotte non entrano nello storico e non toccano i rating; quelle già
  concluse restano salvate.

Messaggi d'errore e avanzamento vanno su **stderr**, i risultati su **stdout**: si possono usare
in pipe (`proxy-finder list | head`, `proxy-finder export | xargs …`).

## Codici di uscita

| Codice | Significato |
|---|---|
| 0 | tutto ok |
| 1 | errore (rete, file, nessun proxy disponibile…) |
| 2 | uso sbagliato (comando, opzione o valore non valido) |
| 130 | annullato con Ctrl+C |

## Rating

Ogni `test` aggiorna il rating delle sorgenti che elencano ciascun proxy provato:

```
score  = clamp(1 − latenza_ms/5000, 0.05, 1) × (1 se regge https, 0.5 se solo http)
rating = (somma degli score dei buoni + prior × 5) / (proxy provati + 5)
```

Un proxy buono alza il rating tanto più quanto è veloce; uno morto lo abbassa. `recheck`, `relay`
e le prove annullate non lo toccano. Dettagli ed esempio numerico in `GUIDA.md`.

L'ordine `priority` (anche in `auto`) moltiplica il rating della sorgente per quello della rete
del proxy (ASN e /24, vedi `nets`), imparato allo stesso modo dalle prime prove.

## Esempi di automazione

```bash
# ogni ora: nuova lista, 128 prove, toglie i morti, esporta
proxy-finder download -q && proxy-finder test -n 128 -q && proxy-finder recheck -q \
  && proxy-finder prune && proxy-finder export ~/proxies.txt

# solo SOCKS5 italiani e tedeschi, in JSON
proxy-finder list --json | jq '.[] | select(.protocol=="socks5" and (.country=="IT" or .country=="DE"))'

# dati separati per un esperimento
PROXY_FINDER_HOME=/tmp/pf-prova proxy-finder download
```
