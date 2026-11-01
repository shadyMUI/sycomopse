# sycomopse

Peripheral Hub 0.1.0: client Linux Tauri per le periferiche esposte da OpenRazer,
libratbag, UPower e HeadsetControl, audio PipeWire/HRTF e rimappatura software del
mouse. L'app e l'agente sono MIT. Il motore Input Remapper privato e un processo
GPL separato; licenze, attribuzioni e sorgenti corrispondenti sono nel pacchetto
e nel bundle di installazione.

## Installazione Linux

Usare il **bundle completo della propria distro/versione** prodotto dal workflow
`Linux Native Packaging`, non il solo pacchetto GUI generato direttamente da
Tauri. Il bundle contiene un pacchetto nativo DEB, RPM o Arch, installer,
`manifest.json`, `manifest.sha256`, rootfs verificabile e archivi sorgente.
Questa pipeline carica soltanto artefatti di build: non pubblica release, repo
o AUR e non genera chiavi di firma.

Dopo aver estratto il bundle, nella sua directory, come utente della sessione
desktop:

```sh
sha256sum -c manifest.sha256
sh install.sh install
```

L'installer verifica anche checksum e dimensione di **tutti** i file elencati
prima di chiedere l'autorizzazione. I checksum rilevano corruzione/modifiche,
non autenticano un archivio proveniente da una fonte sconosciuta. Gli artefatti
locali non sono automaticamente firmati: eventuali policy native che richiedono
una firma vengono rispettate, senza `--nogpgcheck`, repository non autenticati
o modifiche alle policy. Usare solo artefatti di una build fidata.

Il preflight controlla `os-release`, x86_64, systemd come PID 1, versione del
bundle e provenance dell'immagine. Mostra fonti, pacchetti, servizi ed effetti.
L'autorizzazione sudo/Polkit e limitata alla transazione esplicita di
installazione/riparazione/rimozione; GUI e agente non chiedono privilegi e non
sono eseguiti come root. L'attivazione delle unita utente avviene dopo il ritorno
all'utente normale, mai tramite `systemctl --user` eseguito da root.

Dipendenze online risolte con apt, pacman, dnf o zypper e repository autenticati:
WebKitGTK **4.1**, GTK3/appindicator, Python >=3.10, evdev, psutil, dasbus,
Cairo/PyGObject, Pydantic, packaging, typelib **GtkSource4**, PipeWire,
WirePlumber, libmysofa, UPower, libratbag/ratbagd e OpenRazer daemon/DKMS.
L'engine Input Remapper 2.2.1 e incluso privatamente: nessun AUR per l'utente,
nessun pip globale, build root dell'app/engine o script remoto eseguito via pipe.
Quando evdev della distro non espone `UInput.input_props`, la build include una
copia privata 1.9.2 compilata per il Python/ELF del target e la sua fonte BSD.

Gli eventuali repo OpenRazer sono quelli upstream ufficiali: PPA stable su
Ubuntu, OBS hardware:razer su Debian/Fedora/openSUSE, pacchetti ufficiali su
Arch/CachyOS. Le chiavi **pubbliche upstream esistenti** sono controllate per
fingerprint e le configurazioni aggiunte hanno nomi propri dell'installer.
Configurazioni diverse gia presenti non vengono sovrascritte.

## Consensi E Stato

- PulseAudio non viene sostituito senza consenso; lo stesso vale per il passaggio da pipewire-media-session a WirePlumber.
- ratbagd puo effettuare probe USB/HID: il suo avvio richiede consenso esplicito.
- Gli headers devono corrispondere al kernel realmente in esecuzione. CachyOS usa il `pkgbase` effettivo; Fedora usa il kernel-devel esatto, non kernel-debug-devel.
- Nessun altro kernel viene installato per risolvere automaticamente un mismatch. Headers non disponibili o DKMS fallito restano errori/pending.
- Secure Boot non viene disabilitato. Se attivo, DKMS richiede un consenso separato e l'eventuale firma/enrollment MOK resta da completare con gli strumenti della distro. Non vengono create chiavi.
- Nuovo gruppo plugdev, driver non ancora caricato o accesso udev non aggiornato possono richiedere logout/login, reboot o riconnessione. Lo stato non viene dichiarato pronto prima di questi passaggi.
- Rifiutare un backend conserva gli altri componenti installabili, ma produce uno stato incompleto esplicito, non successo completo.

Per automazione, `--yes` conferma la transazione generale, **non** i consensi
separati. Sono disponibili `--consent-audio-switch`,
`--consent-session-manager`, `--consent-ratbag`, `--consent-repository` e
`--consent-unsigned-dkms`, oltre a `--without-audio` e `--without-openrazer`.
Codici d'uscita: `0` operazione pronta/rimozione riuscita, `2` componenti o
sessione pending, `1` errore. Il report distingue pacchetto installato, servizi
running, pending e fallimento parziale; non nasconde la necessita di riparazione.

**HeadsetControl:** il parser Rust attuale accetta soltanto `api_version`
stringa `1.8` o `1.9`, con conteggio e capabilities leggibili coerenti. Versione
del programma e disponibilita del pacchetto non dimostrano compatibilita.
L'installer preferisce un CLI di sistema che passa `-b -o json`; un candidato
privato viene incluso soltanto dopo controllo ELF e lo stesso probe.
La release congelata 4.2.0 dichiara upstream API 1.6, quindi **non e considerata
conforme**. In assenza di un CLI verificato il componente rimane non
configurato. Parser, filtri vendor e supporto hardware non vengono modificati
dal packaging; non e promessa una lettura batteria per dispositivi non supportati.

## Agente E Recovery

Il pacchetto completo installa:

- GUI `/usr/bin/peripheral-hub`.
- Agente `/usr/bin/peripheral-hub-remapping-agent`, compilato con Cargo `--no-default-features`.
- Unita utente `peripheral-hub-remapping-agent.service`.
- Engine privato e risorse in `/usr/lib/peripheral-hub/remapper/`, wrapper `/usr/lib/peripheral-hub/remapper-service`.
- Unita di sistema `peripheral-hub-input-remapper.service` e bus `io.peripheralhub.InputRemapper` indipendenti dal servizio Input Remapper stock.
- Voce desktop **Ripristina mouse**, che esegue soltanto il recovery non-root dell'app.

Chiudere la GUI o usare Esci non disabilita l'agente. Per interrompere soltanto
le rimappature gestite dall'app:

```sh
peripheral-hub-remapping-agent --disable-all
```

La recovery usa lo stato osservato e un registro privato separato dai profili:
puo rilasciare i preset propri anche se il file dei profili e corrotto o ha una
versione sconosciuta, senza sovrascriverlo. Una disattivazione rimane persistita;
un nuovo **Applica pulsanti** esplicito la rimuove solo dopo conferma e salvataggio.
Il locator dell'agente conserva anche un `XDG_CONFIG_HOME` personalizzato, senza
richiedere che systemd erediti l'ambiente della GUI.

Il backend non prende il controllo di un altro remapper: grab esclusivo occupato
significa errore/Busy, non arresto di un servizio esterno. L'installer non
sovrascrive configurazioni Input Remapper, OpenRazer, ratbag o audio standard.

## Repair E Rimozione

Dalla directory del bundle corrispondente:

```sh
sh install.sh status
sh install.sh repair
sh install.sh uninstall
```

`repair` verifica di nuovo il bundle, risolve i pacchetti e reinstalla il
payload proprio per ripristinare file mancanti, poi ricontrolla import,
servizi, DKMS e fase utente. Usarlo anche dopo il logout/reboot richiesto per
ricontrollare lo stato; niente elevazione implicita al primo avvio della GUI.

La rimozione esegue recovery come utente e ferma/disabilita soltanto agente ed
engine privati. Rimuove soltanto il pacchetto dell'app, non le dipendenze,
OpenRazer, PipeWire, WirePlumber, UPower o ratbagd e non arresta servizi esterni.
Dati e profili utente sono conservati. `uninstall --purge` rimuove anche i file
repository creati dall'installer **solo se ancora invariati**; chiavi pubbliche
importate e gruppi condivisi non vengono rimossi. Il registro di operazione
rimane disponibile per diagnosticare modifiche esterne/fallimenti.

## Target E Validazione

La matrice definisce i seguenti **target di build**, non un generico supporto
"tutto Linux":

| Target | Identita/versione accettata | Pacchetto |
| --- | --- | --- |
| Arch | `ID=arch`, rolling | `.pkg.tar.zst` |
| CachyOS | `ID=cachyos`, rolling, immagine generica x86-64 | `.pkg.tar.zst` |
| Ubuntu LTS | 22.04 jammy, 24.04 noble, 26.04 resolute | DEB |
| Debian | 12 bookworm, 13 trixie | DEB |
| Fedora mantenuta | 44, target con scadenza conservativa nel catalogo | RPM |
| openSUSE Leap | 16.0 | RPM |
| openSUSE Tumbleweed | `ID=opensuse-tumbleweed`, rolling | RPM |

Solo x86_64, sistema mutabile e systemd. ARM, altri init, distro immutabili,
NixOS, Alpine, Void, derivazioni non elencate ed edizioni/versioni EOL sono
escluse. Nuove versioni richiedono una nuova build/test, non un alias ID_LIKE.
Non e disponibile un installer offline completo per kernel arbitrari.

Il workflow usa immagini pulite, ne registra il digest realmente scaricato e
`os-release`, riusa il payload GUI Tauri, aggiunge agente e fetch/staging engine
del componente privato, quindi genera **realmente** il pacchetto nativo tramite
dpkg-deb/dpkg-shlibdeps, makepkg o rpmbuild. `readelf`/`ldd`, requisiti nativi e
import Python sono registrati nel manifest. In un container systemd separato
verifica installazione, hook di upgrade/reinstallazione della stessa 0.1.0,
repair di un file proprio cancellato, DBus privato, agente/recovery come utente,
uninstall e reinstallazione. Nessuna versione Cargo/app viene incrementata
artificialmente per lo smoke.

Un bundle con `native_smoke=not-run` non viene accettato per installare. Gli
artefatti vengono caricati dal workflow soltanto dopo lo smoke riuscito.
**In questa sessione Windows non sono state eseguite build ELF/pacchetti Linux,
transazioni native, VM/login/reboot o prove hardware X11/Wayland.** I target
restano non certificati fino ai rispettivi risultati Linux; lo smoke container
non dimostra compatibilita del kernel/DKMS, accesso a un mouse reale o audio.
Questi limiti restano espliciti nel manifest, anche dopo lo smoke nativo.

Build Linux isolata, come utente normale con Docker gia disponibile:

```sh
python3 packaging/ci/matrix.py --target debian-12 --output packaging/out
```

La preparazione dipendenze root e ammessa soltanto nel container CI esplicito;
app, engine e toolchain sono costruiti/preparati come utente non-root. Lo smoke
systemd e isolato e non monta configurazioni, `/dev/input` o moduli del kernel
host. `--skip-smoke` lascia il bundle intenzionalmente non installabile.

## Test Locali

Test indipendenti da Linux e senza installazioni reali:

```sh
python -m unittest discover -s packaging/tests -v
python packaging/install.py --dry-run --json --fixture packaging/tests/fixtures/cachyos.json --consent-ratbag
python packaging/install.py --dry-run --fixture packaging/tests/fixtures/ubuntu22.json
```

Le fixture coprono distro/versioni, kernel/pkgbase/headers, root/utente,
consensi, Secure Boot, checksum/CAS, percorsi e link, repair/uninstall propri,
unita statiche e contratto JSON HeadsetControl reale. `--fixture` e rifiutato
fuori dal dry-run. Test engine aggiuntivi sono in `packaging/remapper/tests`.

Il bundle Tauri predefinito e ora solo **AppImage**. Rimane una distribuzione
portabile della GUI, non installa agente, servizi, policy DBus, DKMS o backend
nativi e non va descritto come installer completo.
