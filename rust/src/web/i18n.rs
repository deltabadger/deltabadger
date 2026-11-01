//! Rails' I18n as this app uses it: config/locales/*.yml (embedded by build.rs), looked up by full
//! key with a fallback to English, `%{name}` interpolation, and pluralisation with Rails' default
//! one/other rule plus the Russian rule in config/locales/plurals.rb.
//! `t` is the view helper (ActionView's `translate`); `text` is `I18n.t`, which controllers use.

include!(concat!(env!("OUT_DIR"), "/translations.rs"));

pub const DEFAULT: &str = "en";

/// Keys Rails takes from a gem's own locale file, not from config/locales. Pinned by tests/i18n.rs:
/// ActiveModel's "taken" message, and the dotiw gem's unit names (`distance_of_time_in_words`) in the
/// ten of this app's locales the gem has a file for.
const FROM_GEMS: &[(&str, &str)] = &[
    ("en.tracker.import.identity_refused_file", "Nothing imported. This file doesn't match your Binance history. It may be cut short or edited. Download a fresh, full export from Binance and try again."),
    ("en.tracker.import.identity_refused_history", "Nothing imported. Your saved Binance history needs a repair before this file can be added. Updating Deltabadger runs the repair, then try the import again."),
    // D-IMPORT-COUNTS (PLAN-A8-bot-csv): Rust-only, after Rails' import notice.
    ("en.bot.details.stats.import_skipped", "Skipped %{count} row(s): not closed, not this bot's assets or quote, or an asset name that fits more than one."),
    ("en.rules.tile.withdrawal_unconfirmed", "Withdrawal status unconfirmed — not retried, to avoid sending it twice. Check %{exchange}."),
    ("bg.tax_report.warnings.unmatched_swaps", "Страни на замяна без страна в обратната посока; където замените не се облагат, цената на придобиване на изходяща страна не можа да бъде пренесена, а за входяща страна е приета цена на придобиване по пазарна стойност"),
    ("bg.tax_report.warnings.unmatched_swaps_hint", "Синхронизирай или импортирай сметката, която съдържа другата страна на всяка замяна, за да се пренесе цената ѝ на придобиване."),
    ("cs.tax_report.warnings.unmatched_swaps", "Části směn bez části v opačném směru; kde směny nepodléhají zdanění, nebylo možné převést pořizovací cenu odchozí části a u příchozí části se pořizovací cena předpokládala v tržní hodnotě"),
    ("cs.tax_report.warnings.unmatched_swaps_hint", "Synchronizuj nebo importuj účet, na kterém je druhá strana každé směny, aby se její pořizovací cena přenesla."),
    ("da.tax_report.warnings.unmatched_swaps", "Swapdele uden en del i modsat retning; hvor swaps ikke beskattes, kunne anskaffelsesprisen for en udgående del ikke overføres, og for en indgående del blev anskaffelsesprisen sat til markedsværdien"),
    ("da.tax_report.warnings.unmatched_swaps_hint", "Synkroniser eller importér den konto, der har den anden side af hver swap, så anskaffelsesprisen føres over."),
    ("de.tax_report.warnings.unmatched_swaps", "Tauschseiten ohne Gegenseite in die andere Richtung; wo Tauschvorgänge nicht steuerpflichtig sind, konnten die Anschaffungskosten einer abgehenden Seite nicht übertragen werden, und für eine eingehende Seite wurden Anschaffungskosten zum Marktwert angenommen"),
    ("de.tax_report.warnings.unmatched_swaps_hint", "Synchronisiere oder importiere das Konto, das die andere Seite jedes Tauschs enthält, damit seine Anschaffungskosten übertragen werden."),
    ("el.tax_report.warnings.unmatched_swaps", "Σκέλη swap χωρίς σκέλος προς την αντίθετη κατεύθυνση· όπου τα swap δεν φορολογούνται, το κόστος κτήσης ενός εξερχόμενου σκέλους δεν μπόρεσε να μεταφερθεί και το κόστος κτήσης ενός εισερχόμενου σκέλους θεωρήθηκε ίσο με την αγοραία αξία"),
    ("el.tax_report.warnings.unmatched_swaps_hint", "Συγχρόνισε ή εισήγαγε τον λογαριασμό που περιέχει την άλλη πλευρά κάθε swap, ώστε να μεταφερθεί το κόστος κτήσης της."),
    ("en.tax_report.warnings.unmatched_swaps", "Swap legs with no leg going the other way; where swaps are not taxable, an outgoing leg's cost basis could not be carried over, and an incoming leg's cost basis was assumed at market value"),
    ("en.tax_report.warnings.unmatched_swaps_hint", "Sync or import the account that holds the other side of each swap, so its cost basis carries across."),
    ("es.tax_report.warnings.unmatched_swaps", "Partes de swap sin una parte en sentido contrario; donde los swaps no tributan, el coste de adquisición de una parte saliente no pudo trasladarse, y el de una parte entrante se asumió a valor de mercado"),
    ("es.tax_report.warnings.unmatched_swaps_hint", "Sincroniza o importa la cuenta que contiene la otra parte de cada swap, para que su coste de adquisición se traslade."),
    ("fr.tax_report.warnings.unmatched_swaps", "Côtés de swap sans côté dans l’autre sens ; là où les swaps ne sont pas imposables, le coût d’acquisition d’un côté sortant n’a pas pu être reporté, et celui d’un côté entrant a été supposé égal à la valeur de marché"),
    ("fr.tax_report.warnings.unmatched_swaps_hint", "Synchronise ou importe le compte qui détient l’autre côté de chaque swap, pour que son coût d’acquisition soit reporté."),
    ("it.tax_report.warnings.unmatched_swaps", "Lati di swap senza un lato nella direzione opposta; dove gli swap non sono imponibili, non è stato possibile riportare il costo di acquisto di un lato in uscita, e quello di un lato in entrata è stato assunto pari al valore di mercato"),
    ("it.tax_report.warnings.unmatched_swaps_hint", "Sincronizza o importa il conto che contiene l'altro lato di ogni swap, così che il suo costo di acquisto venga riportato."),
    ("nl.tax_report.warnings.unmatched_swaps", "Swapdelen zonder deel in de andere richting; waar swaps niet belast zijn, konden de aanschafkosten van een uitgaand deel niet worden overgedragen, en zijn voor een inkomend deel aanschafkosten tegen marktwaarde aangenomen"),
    ("nl.tax_report.warnings.unmatched_swaps_hint", "Synchroniseer of importeer het account met de andere kant van elke swap, zodat de aanschafkosten ervan worden overgedragen."),
    ("pl.tax_report.warnings.unmatched_swaps", "Strony swapów bez strony w przeciwnym kierunku; tam, gdzie swapy nie podlegają opodatkowaniu, nie udało się przenieść kosztu nabycia strony wychodzącej, a koszt nabycia strony przychodzącej przyjęto według wartości rynkowej"),
    ("pl.tax_report.warnings.unmatched_swaps_hint", "Zsynchronizuj lub zaimportuj konto, na którym znajduje się druga strona każdego swapu, aby jej koszt nabycia został przeniesiony."),
    ("pt.tax_report.warnings.unmatched_swaps", "Partes de swap sem uma parte no sentido oposto; onde os swaps não são tributáveis, o custo de aquisição de uma parte de saída não pôde ser transportado, e o de uma parte de entrada foi assumido pelo valor de mercado"),
    ("pt.tax_report.warnings.unmatched_swaps_hint", "Sincroniza ou importa a conta que contém o outro lado de cada swap, para que o seu custo de aquisição seja transportado."),
    ("sk.tax_report.warnings.unmatched_swaps", "Časti výmen bez časti v opačnom smere; kde výmeny nepodliehajú zdaneniu, nebolo možné preniesť obstarávaciu cenu odchádzajúcej časti a pri prichádzajúcej časti sa obstarávacia cena predpokladala v trhovej hodnote"),
    ("sk.tax_report.warnings.unmatched_swaps_hint", "Synchronizuj alebo importuj účet, na ktorom je druhá strana každej výmeny, aby sa jej obstarávacia cena preniesla."),
    ("sv.tax_report.warnings.unmatched_swaps", "Delar av byten utan motsvarande del i andra riktningen; där byten inte beskattas kunde omkostnadsbeloppet för en utgående del inte föras över, och för en inkommande del antogs omkostnadsbeloppet vara marknadsvärdet"),
    ("sv.tax_report.warnings.unmatched_swaps_hint", "Synka eller importera kontot som har andra sidan av varje byte, så att omkostnadsbeloppet förs över."),
    // D-TAX-US-WALLET: the US report is in English only.
    ("en.tax_report.warnings.wallet_allocation", "Cost basis held on 1 January 2025, placed on the accounts that held the units then (Rev. Proc. 2024-28): this report's reconstruction, not an allocation you documented before 2025"),
    ("en.tax_report.warnings.wallet_allocation_hint", "Each line: account, asset, units, cost basis, acquired. If you documented an allocation before 1 January 2025, file by yours instead."),
    ("en.tax_report.warnings.wallet_allocation_unsynced", "unsynced"),
    ("en.errors.messages.taken", "has already been taken"),
    // A12-2 (PLAN-A12-tax-reports): Rust-only warning blocks of the tax report, in every report locale: D-TAX-UNCOVERED,
    // D-TAX-UNLINKED-DISCLOSED, D-TAX-LOST, D-TAX-FEES-DISCLOSED.
    ("bg.tax_report.warnings.coin_fees", "Такси, платени в актив и неотнесени към сделка"),
    ("bg.tax_report.warnings.coin_fees_hint_disposal", "Всяка е отчетена по-горе като разпореждане по пазарната ѝ стойност. Провери всяка спрямо извлечението от твоята борса."),
    ("bg.tax_report.warnings.coin_fees_hint_no_proceeds", "Всяка е приспадната от най-стария ти лот от този актив без постъпления. Провери всяка спрямо извлечението от твоята борса: цената на придобиване при последващи продажби на този актив може да се различава до стойността на таксата."),
    ("bg.tax_report.warnings.lost", "Активи, отбелязани като изгубени; цената им на придобиване е извадена от наличностите ти и за тях не се отчита разпореждане"),
    ("bg.tax_report.warnings.lost_hint", "Дали такава загуба може да се приспадне, зависи от правилата в твоята държава. Провери, преди да я заявиш."),
    ("bg.tax_report.warnings.uncovered_sales", "Продажби на повече, отколкото съдържа синхронизираната ти история; частта, която не е покрита от покупка, е отчетена с нулева цена на придобиване"),
    ("bg.tax_report.warnings.uncovered_sales_hint", "Свържи или импортирай сметката, от която идват тези активи, или свържи депозита, с който са постъпили, за да се използва действителната цена на придобиване при тези продажби."),
    ("bg.tax_report.warnings.unlinked_withdrawals", "Тегления, несвързани с депозит; всяко се третира като прехвърляне към твой собствен портфейл, а не като продажба"),
    ("bg.tax_report.warnings.unlinked_withdrawals_hint", "Свържи всяко със съответния депозит в Тракера. Теглене, което е било продажба, плащане или подарък, е разпореждане, което този отчет не включва."),
    ("cs.tax_report.warnings.coin_fees", "Poplatky zaplacené v aktivu a nepřiřazené k obchodu"),
    ("cs.tax_report.warnings.coin_fees_hint_disposal", "Každý je výše vykázán jako zcizení v tržní hodnotě. Ověř každý podle výpisu ze své burzy."),
    ("cs.tax_report.warnings.coin_fees_hint_no_proceeds", "Každý byl odečten z tvé nejstarší pozice v tomto aktivu bez příjmu. Ověř každý podle výpisu ze své burzy: pořizovací cena pozdějších prodejů tohoto aktiva se může lišit až o hodnotu poplatku."),
    ("cs.tax_report.warnings.lost", "Aktiva označená jako ztracená; jejich pořizovací cena byla odečtena z tvých pozic a žádné zcizení se u nich nevykazuje"),
    ("cs.tax_report.warnings.lost_hint", "Zda lze takovou ztrátu odečíst, závisí na pravidlech tvé země. Ověř si to, než ji uplatníš."),
    ("cs.tax_report.warnings.uncovered_sales", "Prodeje převyšující to, co obsahuje tvoje synchronizovaná historie; část, kterou nekryje žádný nákup, je vykázána s nulovou pořizovací cenou"),
    ("cs.tax_report.warnings.uncovered_sales_hint", "Připoj nebo importuj účet, ze kterého tato aktiva pocházejí, nebo propoj vklad, kterým přišla, aby se u těchto prodejů použila skutečná pořizovací cena."),
    ("cs.tax_report.warnings.unlinked_withdrawals", "Výběry nepropojené s vkladem; každý se považuje za převod do tvé vlastní peněženky, ne za prodej"),
    ("cs.tax_report.warnings.unlinked_withdrawals_hint", "Propoj každý s odpovídajícím vkladem v Trackeru. Výběr, který byl prodejem, platbou nebo darem, je zcizení, které tento přehled nezahrnuje."),
    ("da.tax_report.warnings.coin_fees", "Gebyrer betalt i et aktiv og ikke knyttet til en handel"),
    ("da.tax_report.warnings.coin_fees_hint_disposal", "Hvert gebyr er opgjort ovenfor som en afståelse til markedsværdien. Tjek hvert gebyr mod din børs' opgørelse."),
    ("da.tax_report.warnings.coin_fees_hint_no_proceeds", "Hvert gebyr er trukket fra din ældste post af aktivet uden afståelsessum. Tjek hvert gebyr mod din børs' opgørelse: anskaffelsesprisen for senere salg af aktivet kan afvige med op til gebyrets værdi."),
    ("da.tax_report.warnings.lost", "Aktiver markeret som tabt; deres anskaffelsespris er taget ud af din beholdning, og der opgøres ingen afståelse for dem"),
    ("da.tax_report.warnings.lost_hint", "Om et sådant tab kan fratrækkes, afhænger af reglerne i dit land. Tjek det, før du gør det gældende."),
    ("da.tax_report.warnings.uncovered_sales", "Salg af mere, end din synkroniserede historik indeholder; den del, som intet køb dækker, opgøres med en anskaffelsespris på nul"),
    ("da.tax_report.warnings.uncovered_sales_hint", "Forbind eller importér den konto, aktiverne kom fra, eller tilknyt den indbetaling, de kom ind med, så salgene får deres faktiske anskaffelsespris."),
    ("da.tax_report.warnings.unlinked_withdrawals", "Udbetalinger, der ikke er knyttet til en indbetaling; hver behandles som en flytning til en wallet, du selv ejer, ikke som et salg"),
    ("da.tax_report.warnings.unlinked_withdrawals_hint", "Knyt hver enkelt til den tilsvarende indbetaling i Tracker. En udbetaling, der var et salg, en betaling eller en gave, er en afståelse, som denne rapport ikke medtager."),
    ("de.tax_report.warnings.coin_fees", "In einem Asset gezahlte Gebühren, die keinem Trade zugeordnet wurden"),
    ("de.tax_report.warnings.coin_fees_hint_disposal", "Jede ist oben als Veräußerung zum Marktwert ausgewiesen. Prüfe jede anhand der Abrechnung deiner Börse."),
    ("de.tax_report.warnings.coin_fees_hint_no_proceeds", "Jede wurde ohne Erlös von deinem ältesten Bestand dieses Assets abgezogen. Prüfe jede anhand der Abrechnung deiner Börse: Die Anschaffungskosten späterer Verkäufe dieses Assets können um bis zum Wert der Gebühr abweichen."),
    ("de.tax_report.warnings.lost", "Als verloren markierte Assets; ihre Anschaffungskosten wurden aus deinem Bestand ausgebucht, und für sie wird keine Veräußerung ausgewiesen"),
    ("de.tax_report.warnings.lost_hint", "Ob ein solcher Verlust abziehbar ist, hängt von den Regeln deines Landes ab. Prüfe das, bevor du ihn geltend machst."),
    ("de.tax_report.warnings.uncovered_sales", "Verkäufe von mehr, als deine synchronisierte Historie enthält; der Teil, den kein Kauf abdeckt, wird mit Anschaffungskosten von null ausgewiesen"),
    ("de.tax_report.warnings.uncovered_sales_hint", "Verbinde oder importiere das Konto, von dem diese Assets stammen, oder verknüpfe die Einzahlung, mit der sie hereinkamen, damit diese Verkäufe ihre tatsächlichen Anschaffungskosten erhalten."),
    ("de.tax_report.warnings.unlinked_withdrawals", "Nicht mit einer Einzahlung verknüpfte Auszahlungen; jede wird als Übertrag auf eine eigene Wallet behandelt, nicht als Verkauf"),
    ("de.tax_report.warnings.unlinked_withdrawals_hint", "Verknüpfe jede im Tracker mit der passenden Einzahlung. Eine Auszahlung, die ein Verkauf, eine Zahlung oder eine Schenkung war, ist eine Veräußerung, die dieser Bericht nicht enthält."),
    ("el.tax_report.warnings.coin_fees", "Χρεώσεις που πληρώθηκαν σε στοιχείο και δεν αντιστοιχίστηκαν σε συναλλαγή"),
    ("el.tax_report.warnings.coin_fees_hint_disposal", "Καθεμία αναφέρεται παραπάνω ως διάθεση στην αγοραία αξία της. Έλεγξε καθεμία με την κατάσταση του ανταλλακτηρίου σου."),
    ("el.tax_report.warnings.coin_fees_hint_no_proceeds", "Καθεμία αφαιρέθηκε από την παλαιότερη παρτίδα σου αυτού του στοιχείου χωρίς έσοδο. Έλεγξε καθεμία με την κατάσταση του ανταλλακτηρίου σου: το κόστος κτήσης μεταγενέστερων πωλήσεων αυτού του στοιχείου μπορεί να διαφέρει έως την αξία της χρέωσης."),
    ("el.tax_report.warnings.lost", "Στοιχεία που σημειώθηκαν ως χαμένα· το κόστος κτήσης τους αφαιρέθηκε από τις θέσεις σου και δεν αναφέρεται καμία διάθεση γι’ αυτά"),
    ("el.tax_report.warnings.lost_hint", "Το αν μια τέτοια ζημία εκπίπτει εξαρτάται από τους κανόνες της χώρας σου. Έλεγξέ το πριν τη δηλώσεις."),
    ("el.tax_report.warnings.uncovered_sales", "Πωλήσεις μεγαλύτερες από όσα περιέχει το συγχρονισμένο ιστορικό σου· το μέρος που δεν καλύπτει καμία αγορά αναφέρεται με μηδενικό κόστος κτήσης"),
    ("el.tax_report.warnings.uncovered_sales_hint", "Σύνδεσε ή εισήγαγε τον λογαριασμό από τον οποίο προήλθαν αυτά τα στοιχεία ή σύνδεσε την κατάθεση με την οποία μπήκαν, ώστε αυτές οι πωλήσεις να έχουν το πραγματικό τους κόστος κτήσης."),
    ("el.tax_report.warnings.unlinked_withdrawals", "Αναλήψεις που δεν συνδέθηκαν με κατάθεση· καθεμία αντιμετωπίζεται ως μεταφορά σε δικό σου πορτοφόλι, όχι ως πώληση"),
    ("el.tax_report.warnings.unlinked_withdrawals_hint", "Σύνδεσε καθεμία με την αντίστοιχη κατάθεση στο Tracker. Μια ανάληψη που ήταν πώληση, πληρωμή ή δώρο είναι διάθεση που αυτή η αναφορά δεν περιλαμβάνει."),
    ("en.tax_report.warnings.coin_fees", "Fees paid in an asset and not matched to a trade"),
    ("en.tax_report.warnings.coin_fees_hint_disposal", "Each is reported above as a disposal at its market value. Check each one against your exchange's statement."),
    ("en.tax_report.warnings.coin_fees_hint_no_proceeds", "Each was taken out of your oldest holding of that asset at zero proceeds. Check each one against your exchange's statement: the cost basis of later sales of that asset may be off by up to the fee's value."),
    ("en.tax_report.warnings.lost", "Assets marked as lost; their cost basis was taken out of your holdings and no disposal is reported for them"),
    ("en.tax_report.warnings.lost_hint", "Whether a loss like this can be deducted depends on your country's rules. Check before claiming it."),
    ("en.tax_report.warnings.uncovered_sales", "Sales of more than your synced history holds; the part no purchase covers is reported at a cost basis of zero"),
    ("en.tax_report.warnings.uncovered_sales_hint", "Connect or import the account these assets came from, or link the deposit that brought them in, so these sales carry their real cost basis."),
    ("en.tax_report.warnings.unlinked_withdrawals", "Withdrawals not linked to a deposit; each is treated as a move to a wallet of your own, not as a sale"),
    ("en.tax_report.warnings.unlinked_withdrawals_hint", "Link each one to its matching deposit in the tracker. A withdrawal that was a sale, a payment or a gift is a disposal this report does not include."),
    ("es.tax_report.warnings.coin_fees", "Comisiones pagadas en un activo y no asignadas a una operación"),
    ("es.tax_report.warnings.coin_fees_hint_disposal", "Cada una figura arriba como una transmisión a su valor de mercado. Comprueba cada una con el extracto de tu exchange."),
    ("es.tax_report.warnings.coin_fees_hint_no_proceeds", "Cada una se descontó de tu lote más antiguo de ese activo sin importe de transmisión. Comprueba cada una con el extracto de tu exchange: el coste de adquisición de las ventas posteriores de este activo puede variar hasta el valor de la comisión."),
    ("es.tax_report.warnings.lost", "Activos marcados como perdidos; su coste de adquisición se retiró de tus tenencias y no se declara ninguna transmisión por ellos"),
    ("es.tax_report.warnings.lost_hint", "Que una pérdida así sea deducible depende de las normas de tu país. Compruébalo antes de declararla."),
    ("es.tax_report.warnings.uncovered_sales", "Ventas por más de lo que contiene tu historial sincronizado; la parte que ninguna compra cubre se declara con un coste de adquisición de cero"),
    ("es.tax_report.warnings.uncovered_sales_hint", "Conecta o importa la cuenta de la que proceden estos activos, o vincula el depósito con el que llegaron, para que estas ventas lleven su coste de adquisición real."),
    ("es.tax_report.warnings.unlinked_withdrawals", "Retiros no vinculados a un depósito; cada uno se trata como un traspaso a una cartera propia, no como una venta"),
    ("es.tax_report.warnings.unlinked_withdrawals_hint", "Vincula cada uno con su depósito correspondiente en el Tracker. Un retiro que fue una venta, un pago o una donación es una transmisión que este informe no incluye."),
    ("fr.tax_report.warnings.coin_fees", "Frais payés dans un actif et non rattachés à un ordre"),
    ("fr.tax_report.warnings.coin_fees_hint_disposal", "Chacun figure ci-dessus comme une cession à sa valeur de marché. Vérifie chacun avec le relevé de ta plateforme."),
    ("fr.tax_report.warnings.coin_fees_hint_no_proceeds", "Chacun a été déduit de ton lot le plus ancien de cet actif sans produit de cession. Vérifie chacun avec le relevé de ta plateforme : le coût d’acquisition des ventes ultérieures de cet actif peut différer jusqu’à la valeur des frais."),
    ("fr.tax_report.warnings.lost", "Actifs marqués comme perdus ; leur coût d’acquisition a été retiré de tes avoirs et aucune cession n’est déclarée pour eux"),
    ("fr.tax_report.warnings.lost_hint", "La déductibilité d’une telle perte dépend des règles de ton pays. Vérifie avant de la faire valoir."),
    ("fr.tax_report.warnings.uncovered_sales", "Ventes supérieures à ce que contient ton historique synchronisé ; la part qu’aucun achat ne couvre est déclarée avec un coût d’acquisition nul"),
    ("fr.tax_report.warnings.uncovered_sales_hint", "Connecte ou importe le compte d’où viennent ces actifs, ou associe le dépôt par lequel ils sont arrivés, pour que ces ventes portent leur coût d’acquisition réel."),
    ("fr.tax_report.warnings.unlinked_withdrawals", "Retraits non associés à un dépôt ; chacun est traité comme un transfert vers un portefeuille à toi, pas comme une vente"),
    ("fr.tax_report.warnings.unlinked_withdrawals_hint", "Associe chacun au dépôt correspondant dans le Tracker. Un retrait qui était une vente, un paiement ou un don est une cession que ce rapport n’inclut pas."),
    ("it.tax_report.warnings.coin_fees", "Commissioni pagate in un asset e non abbinate a un'operazione"),
    ("it.tax_report.warnings.coin_fees_hint_disposal", "Ciascuna è riportata sopra come cessione al suo valore di mercato. Verifica ciascuna con l'estratto del tuo exchange."),
    ("it.tax_report.warnings.coin_fees_hint_no_proceeds", "Ciascuna è stata detratta dal tuo lotto più vecchio di quell'asset senza corrispettivo. Verifica ciascuna con l'estratto del tuo exchange: il costo di acquisto delle vendite successive di questo asset può differire fino al valore della commissione."),
    ("it.tax_report.warnings.lost", "Asset contrassegnati come persi; il loro costo di acquisto è stato tolto dalle tue disponibilità e per essi non è riportata alcuna cessione"),
    ("it.tax_report.warnings.lost_hint", "La deducibilità di una perdita simile dipende dalle regole del tuo paese. Verifica prima di farla valere."),
    ("it.tax_report.warnings.uncovered_sales", "Vendite superiori a quanto contiene il tuo storico sincronizzato; la parte non coperta da alcun acquisto è riportata con un costo di acquisto pari a zero"),
    ("it.tax_report.warnings.uncovered_sales_hint", "Collega o importa il conto da cui provengono questi asset, oppure collega il deposito con cui sono arrivati, così che queste vendite abbiano il loro costo di acquisto reale."),
    ("it.tax_report.warnings.unlinked_withdrawals", "Prelievi non collegati a un deposito; ciascuno è trattato come un trasferimento verso un tuo wallet, non come una vendita"),
    ("it.tax_report.warnings.unlinked_withdrawals_hint", "Collega ciascuno al deposito corrispondente nel Tracker. Un prelievo che era una vendita, un pagamento o un regalo è una cessione che questo report non include."),
    ("nl.tax_report.warnings.coin_fees", "Kosten betaald in een asset die niet aan een transactie zijn gekoppeld"),
    ("nl.tax_report.warnings.coin_fees_hint_disposal", "Elk is hierboven gerapporteerd als vervreemding tegen de marktwaarde. Controleer elk met het overzicht van je exchange."),
    ("nl.tax_report.warnings.coin_fees_hint_no_proceeds", "Elk is zonder opbrengst van je oudste positie in dat asset afgetrokken. Controleer elk met het overzicht van je exchange: de aanschafkosten van latere verkopen van dit asset kunnen tot de waarde van de kosten afwijken."),
    ("nl.tax_report.warnings.lost", "Als verloren gemarkeerde assets; hun aanschafkosten zijn uit je bezit gehaald en er wordt geen vervreemding voor gerapporteerd"),
    ("nl.tax_report.warnings.lost_hint", "Of zo'n verlies aftrekbaar is, hangt af van de regels in je land. Controleer dit voordat je het opvoert."),
    ("nl.tax_report.warnings.uncovered_sales", "Verkopen van meer dan je gesynchroniseerde geschiedenis bevat; het deel dat geen aankoop dekt, wordt gerapporteerd met aanschafkosten van nul"),
    ("nl.tax_report.warnings.uncovered_sales_hint", "Koppel of importeer het account waar deze assets vandaan kwamen, of koppel de storting waarmee ze binnenkwamen, zodat deze verkopen hun werkelijke aanschafkosten krijgen."),
    ("nl.tax_report.warnings.unlinked_withdrawals", "Opnames die niet aan een storting zijn gekoppeld; elke opname wordt behandeld als een overboeking naar een eigen wallet, niet als een verkoop"),
    ("nl.tax_report.warnings.unlinked_withdrawals_hint", "Koppel elke opname in de Tracker aan de bijbehorende storting. Een opname die een verkoop, een betaling of een schenking was, is een vervreemding die dit rapport niet bevat."),
    ("pl.tax_report.warnings.coin_fees", "Opłaty uiszczone w aktywie i nieprzypisane do transakcji"),
    ("pl.tax_report.warnings.coin_fees_hint_disposal", "Każda jest wykazana powyżej jako zbycie według wartości rynkowej. Sprawdź każdą z wyciągiem z Twojej giełdy."),
    ("pl.tax_report.warnings.coin_fees_hint_no_proceeds", "Każdą odjęto od Twojej najstarszej partii tego aktywa bez przychodu. Sprawdź każdą z wyciągiem z Twojej giełdy: koszt nabycia późniejszych sprzedaży tego aktywa może się różnić maksymalnie o wartość opłaty."),
    ("pl.tax_report.warnings.lost", "Aktywa oznaczone jako utracone; ich koszt nabycia usunięto z Twoich zasobów i nie wykazano dla nich żadnego zbycia"),
    ("pl.tax_report.warnings.lost_hint", "To, czy taką stratę można odliczyć, zależy od przepisów Twojego kraju. Sprawdź to przed jej wykazaniem."),
    ("pl.tax_report.warnings.uncovered_sales", "Sprzedaże przekraczające to, co zawiera Twoja zsynchronizowana historia; część nieobjęta żadnym zakupem jest wykazana z zerowym kosztem nabycia"),
    ("pl.tax_report.warnings.uncovered_sales_hint", "Połącz lub zaimportuj konto, z którego pochodzą te aktywa, albo połącz wpłatę, przez którą trafiły na Twoje konto, aby te sprzedaże miały rzeczywisty koszt nabycia."),
    ("pl.tax_report.warnings.unlinked_withdrawals", "Wypłaty niepołączone z wpłatą; każdą traktuje się jako przeniesienie do własnego portfela, a nie jako sprzedaż"),
    ("pl.tax_report.warnings.unlinked_withdrawals_hint", "Połącz każdą z odpowiadającą jej wpłatą w Trackerze. Wypłata, która była sprzedażą, płatnością lub darowizną, to zbycie, którego ten raport nie obejmuje."),
    ("pt.tax_report.warnings.coin_fees", "Comissões pagas num ativo e não associadas a uma operação"),
    ("pt.tax_report.warnings.coin_fees_hint_disposal", "Cada uma está declarada acima como alienação pelo seu valor de mercado. Confirma cada uma com o extrato da tua exchange."),
    ("pt.tax_report.warnings.coin_fees_hint_no_proceeds", "Cada uma foi deduzida do teu lote mais antigo desse ativo sem valor de realização. Confirma cada uma com o extrato da tua exchange: o custo de aquisição das vendas seguintes deste ativo pode diferir até ao valor da comissão."),
    ("pt.tax_report.warnings.lost", "Ativos marcados como perdidos; o seu custo de aquisição foi retirado das tuas posições e nenhuma alienação é declarada para eles"),
    ("pt.tax_report.warnings.lost_hint", "Se uma perda destas pode ser deduzida depende das regras do teu país. Confirma antes de a declarar."),
    ("pt.tax_report.warnings.uncovered_sales", "Vendas superiores ao que o teu histórico sincronizado contém; a parte que nenhuma compra cobre é declarada com um custo de aquisição de zero"),
    ("pt.tax_report.warnings.uncovered_sales_hint", "Liga ou importa a conta de onde vieram estes ativos, ou associa o depósito pelo qual entraram, para que estas vendas tenham o seu custo de aquisição real."),
    ("pt.tax_report.warnings.unlinked_withdrawals", "Levantamentos não associados a um depósito; cada um é tratado como uma transferência para uma carteira tua, não como uma venda"),
    ("pt.tax_report.warnings.unlinked_withdrawals_hint", "Associa cada um ao depósito correspondente no Tracker. Um levantamento que foi uma venda, um pagamento ou uma doação é uma alienação que este relatório não inclui."),
    ("sk.tax_report.warnings.coin_fees", "Poplatky zaplatené v aktíve a nepriradené k obchodu"),
    ("sk.tax_report.warnings.coin_fees_hint_disposal", "Každý je vyššie vykázaný ako scudzenie v trhovej hodnote. Over každý podľa výpisu zo svojej burzy."),
    ("sk.tax_report.warnings.coin_fees_hint_no_proceeds", "Každý bol odpočítaný z tvojej najstaršej pozície v tomto aktíve bez príjmu. Over každý podľa výpisu zo svojej burzy: obstarávacia cena neskorších predajov tohto aktíva sa môže líšiť až o hodnotu poplatku."),
    ("sk.tax_report.warnings.lost", "Aktíva označené ako stratené; ich obstarávacia cena bola odpočítaná z tvojich pozícií a žiadne scudzenie sa pri nich nevykazuje"),
    ("sk.tax_report.warnings.lost_hint", "Či sa dá takáto strata odpočítať, závisí od pravidiel tvojej krajiny. Over si to skôr, než ju uplatníš."),
    ("sk.tax_report.warnings.uncovered_sales", "Predaje prevyšujúce to, čo obsahuje tvoja synchronizovaná história; časť, ktorú nepokrýva žiadny nákup, je vykázaná s nulovou obstarávacou cenou"),
    ("sk.tax_report.warnings.uncovered_sales_hint", "Pripoj alebo importuj účet, z ktorého tieto aktíva pochádzajú, alebo prepoj vklad, ktorým prišli, aby sa pri týchto predajoch použila skutočná obstarávacia cena."),
    ("sk.tax_report.warnings.unlinked_withdrawals", "Výbery neprepojené s vkladom; každý sa považuje za presun do tvojej vlastnej peňaženky, nie za predaj"),
    ("sk.tax_report.warnings.unlinked_withdrawals_hint", "Prepoj každý so zodpovedajúcim vkladom v Trackeri. Výber, ktorý bol predajom, platbou alebo darom, je scudzenie, ktoré tento prehľad nezahŕňa."),
    ("sv.tax_report.warnings.coin_fees", "Avgifter betalda i en tillgång som inte kopplats till en affär"),
    ("sv.tax_report.warnings.coin_fees_hint_disposal", "Varje avgift redovisas ovan som en avyttring till marknadsvärdet. Stäm av varje avgift mot underlaget från din börs."),
    ("sv.tax_report.warnings.coin_fees_hint_no_proceeds", "Varje avgift drogs från din äldsta post av tillgången utan ersättning. Stäm av varje avgift mot underlaget från din börs: omkostnadsbeloppet för senare försäljningar av tillgången kan avvika med upp till avgiftens värde."),
    ("sv.tax_report.warnings.lost", "Tillgångar markerade som förlorade; deras omkostnadsbelopp har tagits bort från ditt innehav och ingen avyttring redovisas för dem"),
    ("sv.tax_report.warnings.lost_hint", "Om en sådan förlust är avdragsgill beror på reglerna i ditt land. Kontrollera det innan du gör avdrag för den."),
    ("sv.tax_report.warnings.uncovered_sales", "Försäljningar av mer än din synkade historik innehåller; den del som inget köp täcker redovisas med ett omkostnadsbelopp på noll"),
    ("sv.tax_report.warnings.uncovered_sales_hint", "Anslut eller importera kontot som tillgångarna kom från, eller länka insättningen som förde in dem, så att försäljningarna får sitt verkliga omkostnadsbelopp."),
    ("sv.tax_report.warnings.unlinked_withdrawals", "Uttag som inte länkats till en insättning; varje uttag behandlas som en flytt till en egen plånbok, inte som en försäljning"),
    ("sv.tax_report.warnings.unlinked_withdrawals_hint", "Länka varje uttag till motsvarande insättning i Tracker. Ett uttag som var en försäljning, en betalning eller en gåva är en avyttring som rapporten inte tar med."),
    ("da.datetime.dotiw.days.one", "1 dag"), ("da.datetime.dotiw.days.other", "%{count} dage"), ("da.datetime.dotiw.hours.one", "1 time"),
    ("da.datetime.dotiw.hours.other", "%{count} timer"), ("da.datetime.dotiw.less_than_x", "mindre end %{distance}"),
    ("da.datetime.dotiw.minutes.one", "1 minut"), ("da.datetime.dotiw.minutes.other", "%{count} minutter"), ("da.datetime.dotiw.months.one", "1 måned"),
    ("da.datetime.dotiw.months.other", "%{count} måneder"), ("da.datetime.dotiw.seconds.one", "1 sekund"),
    ("da.datetime.dotiw.seconds.other", "%{count} sekunder"), ("da.datetime.dotiw.weeks.one", "1 uge"), ("da.datetime.dotiw.weeks.other", "%{count} uger"),
    ("da.datetime.dotiw.years", "%{count} år"), ("de.datetime.dotiw.days.one", "1 Tag"), ("de.datetime.dotiw.days.other", "%{count} Tage"),
    ("de.datetime.dotiw.hours.one", "1 Stunde"), ("de.datetime.dotiw.hours.other", "%{count} Stunden"),
    ("de.datetime.dotiw.less_than_x", "weniger als %{distance}"), ("de.datetime.dotiw.minutes.one", "1 Minute"),
    ("de.datetime.dotiw.minutes.other", "%{count} Minuten"), ("de.datetime.dotiw.months.one", "1 Monat"),
    ("de.datetime.dotiw.months.other", "%{count} Monate"), ("de.datetime.dotiw.seconds.one", "1 Sekunde"),
    ("de.datetime.dotiw.seconds.other", "%{count} Sekunden"), ("de.datetime.dotiw.weeks.one", "1 Woche"),
    ("de.datetime.dotiw.weeks.other", "%{count} Wochen"), ("de.datetime.dotiw.years.one", "1 Jahr"), ("de.datetime.dotiw.years.other", "%{count} Jahre"),
    ("en.datetime.dotiw.days.one", "1 day"), ("en.datetime.dotiw.days.other", "%{count} days"), ("en.datetime.dotiw.hours.one", "1 hour"),
    ("en.datetime.dotiw.hours.other", "%{count} hours"), ("en.datetime.dotiw.less_than_x", "less than %{distance}"),
    ("en.datetime.dotiw.minutes.one", "1 minute"), ("en.datetime.dotiw.minutes.other", "%{count} minutes"), ("en.datetime.dotiw.months.one", "1 month"),
    ("en.datetime.dotiw.months.other", "%{count} months"), ("en.datetime.dotiw.seconds.one", "1 second"),
    ("en.datetime.dotiw.seconds.other", "%{count} seconds"), ("en.datetime.dotiw.weeks.one", "1 week"), ("en.datetime.dotiw.weeks.other", "%{count} weeks"),
    ("en.datetime.dotiw.years.one", "1 year"), ("en.datetime.dotiw.years.other", "%{count} years"), ("es.datetime.dotiw.days.one", "un día"),
    ("es.datetime.dotiw.days.other", "%{count} días"), ("es.datetime.dotiw.hours.one", "una hora"), ("es.datetime.dotiw.hours.other", "%{count} horas"),
    ("es.datetime.dotiw.less_than_x", "menos de %{distance}"), ("es.datetime.dotiw.minutes.one", "un minuto"),
    ("es.datetime.dotiw.minutes.other", "%{count} minutos"), ("es.datetime.dotiw.months.one", "un mes"), ("es.datetime.dotiw.months.other", "%{count} meses"),
    ("es.datetime.dotiw.seconds.one", "un segundo"), ("es.datetime.dotiw.seconds.other", "%{count} segundos"), ("es.datetime.dotiw.weeks.one", "una semana"),
    ("es.datetime.dotiw.weeks.other", "%{count} semanas"), ("es.datetime.dotiw.years.one", "un año"), ("es.datetime.dotiw.years.other", "%{count} años"),
    ("fr.datetime.dotiw.days.one", "1 jour"), ("fr.datetime.dotiw.days.other", "%{count} jours"), ("fr.datetime.dotiw.hours.one", "1 heure"),
    ("fr.datetime.dotiw.hours.other", "%{count} heures"), ("fr.datetime.dotiw.less_than_x", "moins de %{distance}"),
    ("fr.datetime.dotiw.minutes.one", "1 minute"), ("fr.datetime.dotiw.minutes.other", "%{count} minutes"), ("fr.datetime.dotiw.months.one", "1 mois"),
    ("fr.datetime.dotiw.months.other", "%{count} mois"), ("fr.datetime.dotiw.seconds.one", "1 seconde"),
    ("fr.datetime.dotiw.seconds.other", "%{count} secondes"), ("fr.datetime.dotiw.weeks.one", "1 semaine"),
    ("fr.datetime.dotiw.weeks.other", "%{count} semaines"), ("fr.datetime.dotiw.years.one", "1 an"), ("fr.datetime.dotiw.years.other", "%{count} ans"),
    ("it.datetime.dotiw.days.one", "un giorno"), ("it.datetime.dotiw.days.other", "%{count} giorni"), ("it.datetime.dotiw.hours.one", "una ora"),
    ("it.datetime.dotiw.hours.other", "%{count} ore"), ("it.datetime.dotiw.minutes.one", "un minuto"), ("it.datetime.dotiw.minutes.other", "%{count} minuti"),
    ("it.datetime.dotiw.months.one", "un mese"), ("it.datetime.dotiw.months.other", "%{count} mesi"), ("it.datetime.dotiw.seconds.one", "un secondo"),
    ("it.datetime.dotiw.seconds.other", "%{count} secondi"), ("it.datetime.dotiw.weeks.one", "una settimana"),
    ("it.datetime.dotiw.weeks.other", "%{count} settimane"), ("it.datetime.dotiw.years.one", "un anno"), ("it.datetime.dotiw.years.other", "%{count} anni"),
    ("nl.datetime.dotiw.days.one", "1 dag"), ("nl.datetime.dotiw.days.other", "%{count} dagen"), ("nl.datetime.dotiw.hours.one", "1 uur"),
    ("nl.datetime.dotiw.hours.other", "%{count} uur"), ("nl.datetime.dotiw.less_than_x", "minder dan %{distance}"),
    ("nl.datetime.dotiw.minutes.one", "1 minuut"), ("nl.datetime.dotiw.minutes.other", "%{count} minuten"), ("nl.datetime.dotiw.months.one", "1 maand"),
    ("nl.datetime.dotiw.months.other", "%{count} maanden"), ("nl.datetime.dotiw.seconds.one", "1 seconde"),
    ("nl.datetime.dotiw.seconds.other", "%{count} seconden"), ("nl.datetime.dotiw.weeks.one", "1 week"), ("nl.datetime.dotiw.weeks.other", "%{count} weken"),
    ("nl.datetime.dotiw.years.one", "1 jaar"), ("nl.datetime.dotiw.years.other", "%{count} jaar"), ("pl.datetime.dotiw.days.few", "%{count} dni"),
    ("pl.datetime.dotiw.days.many", "%{count} dni"), ("pl.datetime.dotiw.days.one", "1 dzień"), ("pl.datetime.dotiw.days.other", "%{count} dni"),
    ("pl.datetime.dotiw.hours.few", "%{count} godziny"), ("pl.datetime.dotiw.hours.many", "%{count} godzin"), ("pl.datetime.dotiw.hours.one", "1 godzina"),
    ("pl.datetime.dotiw.hours.other", "%{count} godzin"), ("pl.datetime.dotiw.less_than_x", "mniej niż %{distance}"),
    ("pl.datetime.dotiw.minutes.few", "%{count} minuty"), ("pl.datetime.dotiw.minutes.many", "%{count} minut"), ("pl.datetime.dotiw.minutes.one", "1 minuta"),
    ("pl.datetime.dotiw.minutes.other", "%{count} minut"), ("pl.datetime.dotiw.months.few", "%{count} miesiące"),
    ("pl.datetime.dotiw.months.many", "%{count} miesięcy"), ("pl.datetime.dotiw.months.one", "1 miesiąc"),
    ("pl.datetime.dotiw.months.other", "%{count} miesięcy"), ("pl.datetime.dotiw.seconds.few", "%{count} sekundy"),
    ("pl.datetime.dotiw.seconds.many", "%{count} sekund"), ("pl.datetime.dotiw.seconds.one", "1 sekunda"),
    ("pl.datetime.dotiw.seconds.other", "%{count} sekund"), ("pl.datetime.dotiw.weeks.few", "%{count} tygodnie"),
    ("pl.datetime.dotiw.weeks.many", "%{count} tygodni"), ("pl.datetime.dotiw.weeks.one", "1 tydzień"), ("pl.datetime.dotiw.weeks.other", "%{count} tygodni"),
    ("pl.datetime.dotiw.years.few", "%{count} lata"), ("pl.datetime.dotiw.years.many", "%{count} lat"), ("pl.datetime.dotiw.years.one", "1 rok"),
    ("pl.datetime.dotiw.years.other", "%{count} lat"), ("ru.datetime.dotiw.days.few", "%{count} дня"), ("ru.datetime.dotiw.days.many", "%{count} дней"),
    ("ru.datetime.dotiw.days.one", "%{count} день"), ("ru.datetime.dotiw.days.other", "%{count} дня"), ("ru.datetime.dotiw.hours.few", "%{count} часа"),
    ("ru.datetime.dotiw.hours.many", "%{count} часов"), ("ru.datetime.dotiw.hours.one", "%{count} час"), ("ru.datetime.dotiw.hours.other", "%{count} часа"),
    ("ru.datetime.dotiw.less_than_x", "меньше, чем %{distance}"), ("ru.datetime.dotiw.minutes.few", "%{count} минуты"),
    ("ru.datetime.dotiw.minutes.many", "%{count} минут"), ("ru.datetime.dotiw.minutes.one", "%{count} минута"),
    ("ru.datetime.dotiw.minutes.other", "%{count} минуты"), ("ru.datetime.dotiw.months.few", "%{count} месяца"),
    ("ru.datetime.dotiw.months.many", "%{count} месяцев"), ("ru.datetime.dotiw.months.one", "%{count} месяц"),
    ("ru.datetime.dotiw.months.other", "%{count} месяца"), ("ru.datetime.dotiw.seconds.few", "%{count} секунды"),
    ("ru.datetime.dotiw.seconds.many", "%{count} секунд"), ("ru.datetime.dotiw.seconds.one", "%{count} секунда"),
    ("ru.datetime.dotiw.seconds.other", "%{count} секунды"), ("ru.datetime.dotiw.weeks.few", "%{count} недели"),
    ("ru.datetime.dotiw.weeks.many", "%{count} недель"), ("ru.datetime.dotiw.weeks.one", "%{count} неделя"),
    ("ru.datetime.dotiw.weeks.other", "%{count} недели"), ("ru.datetime.dotiw.years.few", "%{count} года"), ("ru.datetime.dotiw.years.many", "%{count} лет"),
    ("ru.datetime.dotiw.years.one", "%{count} год"), ("ru.datetime.dotiw.years.other", "%{count} года"), ("sv.datetime.dotiw.days.one", "1 dag"),
    ("sv.datetime.dotiw.days.other", "%{count} dagar"), ("sv.datetime.dotiw.hours.one", "1 timme"), ("sv.datetime.dotiw.hours.other", "%{count} timmar"),
    ("sv.datetime.dotiw.less_than_x", "mindre än %{distance}"), ("sv.datetime.dotiw.minutes.one", "1 minut"),
    ("sv.datetime.dotiw.minutes.other", "%{count} minuter"), ("sv.datetime.dotiw.months.one", "1 månad"),
    ("sv.datetime.dotiw.months.other", "%{count} månader"), ("sv.datetime.dotiw.seconds.one", "1 sekund"),
    ("sv.datetime.dotiw.seconds.other", "%{count} sekunder"), ("sv.datetime.dotiw.weeks.one", "1 vecka"),
    ("sv.datetime.dotiw.weeks.other", "%{count} veckor"), ("sv.datetime.dotiw.years.one", "1 år"), ("sv.datetime.dotiw.years.other", "%{count} år"),
    // A12-2b, Poland's PIT-38 workpaper (tax/pl.rs). PIT-38(18) part E's own labels, in Polish in every locale (every
    // locale falls back to these): a form's literal text, as a third-party UI's, is never translated.
    ("en.tax_report.pit38.header", "PIT-38, część E — odpłatne zbycie walut wirtualnych (art. 30b ust. 1a)"),
    ("en.tax_report.pit38.b", "Przychód (b)"),
    ("en.tax_report.pit38.c", "Koszty uzyskania przychodów poniesione w roku podatkowym (c)"),
    ("en.tax_report.pit38.d", "Koszty uzyskania przychodów poniesione w latach ubiegłych i niepotrącone w poprzednim roku podatkowym (d)"),
    ("en.tax_report.pit38.e", "Dochód b – (c + d) (e)"),
    ("en.tax_report.pit38.f", "Koszty uzyskania przychodów, które nie zostały potrącone w roku podatkowym (c + d) – b (f)"),
    ("bg.tax_report.headers.kind", "Вид"),
    ("cs.tax_report.headers.kind", "Druh"),
    ("da.tax_report.headers.kind", "Type"),
    ("de.tax_report.headers.kind", "Art"),
    ("el.tax_report.headers.kind", "Είδος"),
    ("en.tax_report.headers.kind", "Type"),
    ("es.tax_report.headers.kind", "Tipo"),
    ("fr.tax_report.headers.kind", "Type"),
    ("it.tax_report.headers.kind", "Tipo"),
    ("nl.tax_report.headers.kind", "Soort"),
    ("pl.tax_report.headers.kind", "Rodzaj"),
    ("pt.tax_report.headers.kind", "Tipo"),
    ("sk.tax_report.headers.kind", "Druh"),
    ("sv.tax_report.headers.kind", "Typ"),
    ("bg.tax_report.headers.original_amount", "Сума във валутата на сделката"),
    ("cs.tax_report.headers.original_amount", "Částka v měně transakce"),
    ("da.tax_report.headers.original_amount", "Beløb i transaktionsvaluta"),
    ("de.tax_report.headers.original_amount", "Betrag in Transaktionswährung"),
    ("el.tax_report.headers.original_amount", "Ποσό στο νόμισμα της συναλλαγής"),
    ("en.tax_report.headers.original_amount", "Amount in transaction currency"),
    ("es.tax_report.headers.original_amount", "Importe en la moneda de la transacción"),
    ("fr.tax_report.headers.original_amount", "Montant dans la devise de la transaction"),
    ("it.tax_report.headers.original_amount", "Importo nella valuta della transazione"),
    ("nl.tax_report.headers.original_amount", "Bedrag in transactievaluta"),
    ("pl.tax_report.headers.original_amount", "Kwota w walucie transakcji"),
    ("pt.tax_report.headers.original_amount", "Montante na moeda da transação"),
    ("sk.tax_report.headers.original_amount", "Suma v mene transakcie"),
    ("sv.tax_report.headers.original_amount", "Belopp i transaktionsvaluta"),
    ("bg.tax_report.headers.original_currency", "Валута на сделката"),
    ("cs.tax_report.headers.original_currency", "Měna transakce"),
    ("da.tax_report.headers.original_currency", "Transaktionsvaluta"),
    ("de.tax_report.headers.original_currency", "Transaktionswährung"),
    ("el.tax_report.headers.original_currency", "Νόμισμα συναλλαγής"),
    ("en.tax_report.headers.original_currency", "Transaction currency"),
    ("es.tax_report.headers.original_currency", "Moneda de la transacción"),
    ("fr.tax_report.headers.original_currency", "Devise de la transaction"),
    ("it.tax_report.headers.original_currency", "Valuta della transazione"),
    ("nl.tax_report.headers.original_currency", "Transactievaluta"),
    ("pl.tax_report.headers.original_currency", "Waluta transakcji"),
    ("pt.tax_report.headers.original_currency", "Moeda da transação"),
    ("sk.tax_report.headers.original_currency", "Mena transakcie"),
    ("sv.tax_report.headers.original_currency", "Transaktionsvaluta"),
    ("bg.tax_report.headers.nbp_rate", "Среден курс на NBP"),
    ("cs.tax_report.headers.nbp_rate", "Střední kurz NBP"),
    ("da.tax_report.headers.nbp_rate", "NBP's middelkurs"),
    ("de.tax_report.headers.nbp_rate", "NBP-Mittelkurs"),
    ("el.tax_report.headers.nbp_rate", "Μέση ισοτιμία NBP"),
    ("en.tax_report.headers.nbp_rate", "NBP average rate"),
    ("es.tax_report.headers.nbp_rate", "Tipo medio NBP"),
    ("fr.tax_report.headers.nbp_rate", "Cours moyen NBP"),
    ("it.tax_report.headers.nbp_rate", "Cambio medio NBP"),
    ("nl.tax_report.headers.nbp_rate", "NBP-middenkoers"),
    ("pl.tax_report.headers.nbp_rate", "Kurs średni NBP"),
    ("pt.tax_report.headers.nbp_rate", "Taxa média NBP"),
    ("sk.tax_report.headers.nbp_rate", "Stredný kurz NBP"),
    ("sv.tax_report.headers.nbp_rate", "NBP:s mittkurs"),
    ("bg.tax_report.headers.nbp_table_date", "Дата на таблицата на NBP"),
    ("cs.tax_report.headers.nbp_table_date", "Datum tabulky NBP"),
    ("da.tax_report.headers.nbp_table_date", "Dato for NBP-tabellen"),
    ("de.tax_report.headers.nbp_table_date", "Datum der NBP-Tabelle"),
    ("el.tax_report.headers.nbp_table_date", "Ημερομηνία πίνακα NBP"),
    ("en.tax_report.headers.nbp_table_date", "NBP table date"),
    ("es.tax_report.headers.nbp_table_date", "Fecha de la tabla NBP"),
    ("fr.tax_report.headers.nbp_table_date", "Date du tableau NBP"),
    ("it.tax_report.headers.nbp_table_date", "Data della tabella NBP"),
    ("nl.tax_report.headers.nbp_table_date", "Datum NBP-tabel"),
    ("pl.tax_report.headers.nbp_table_date", "Data tabeli NBP"),
    ("pt.tax_report.headers.nbp_table_date", "Data da tabela NBP"),
    ("sk.tax_report.headers.nbp_table_date", "Dátum tabuľky NBP"),
    ("sv.tax_report.headers.nbp_table_date", "Datum för NBP-tabellen"),
    ("bg.tax_report.headers.value_pln", "Сума в PLN"),
    ("cs.tax_report.headers.value_pln", "Částka v PLN"),
    ("da.tax_report.headers.value_pln", "Beløb i PLN"),
    ("de.tax_report.headers.value_pln", "Betrag in PLN"),
    ("el.tax_report.headers.value_pln", "Ποσό σε PLN"),
    ("en.tax_report.headers.value_pln", "Amount in PLN"),
    ("es.tax_report.headers.value_pln", "Importe en PLN"),
    ("fr.tax_report.headers.value_pln", "Montant en PLN"),
    ("it.tax_report.headers.value_pln", "Importo in PLN"),
    ("nl.tax_report.headers.value_pln", "Bedrag in PLN"),
    ("pl.tax_report.headers.value_pln", "Kwota w PLN"),
    ("pt.tax_report.headers.value_pln", "Montante em PLN"),
    ("sk.tax_report.headers.value_pln", "Suma v PLN"),
    ("sv.tax_report.headers.value_pln", "Belopp i PLN"),
    ("bg.tax_report.values.revenue", "Приход"),
    ("cs.tax_report.values.revenue", "Příjem"),
    ("da.tax_report.values.revenue", "Indtægt"),
    ("de.tax_report.values.revenue", "Einnahme"),
    ("el.tax_report.values.revenue", "Έσοδο"),
    ("en.tax_report.values.revenue", "Revenue"),
    ("es.tax_report.values.revenue", "Ingreso"),
    ("fr.tax_report.values.revenue", "Recette"),
    ("it.tax_report.values.revenue", "Ricavo"),
    ("nl.tax_report.values.revenue", "Opbrengst"),
    ("pl.tax_report.values.revenue", "Przychód"),
    ("pt.tax_report.values.revenue", "Receita"),
    ("sk.tax_report.values.revenue", "Príjem"),
    ("sv.tax_report.values.revenue", "Intäkt"),
    ("bg.tax_report.values.cost", "Разход"),
    ("cs.tax_report.values.cost", "Náklad"),
    ("da.tax_report.values.cost", "Omkostning"),
    ("de.tax_report.values.cost", "Kosten"),
    ("el.tax_report.values.cost", "Κόστος"),
    ("en.tax_report.values.cost", "Cost"),
    ("es.tax_report.values.cost", "Coste"),
    ("fr.tax_report.values.cost", "Coût"),
    ("it.tax_report.values.cost", "Costo"),
    ("nl.tax_report.values.cost", "Kosten"),
    ("pl.tax_report.values.cost", "Koszt"),
    ("pt.tax_report.values.cost", "Custo"),
    ("sk.tax_report.values.cost", "Náklad"),
    ("sv.tax_report.values.cost", "Kostnad"),
    ("bg.tax_report.pit38.base", "Данъчна основа, закръглена до цели злоти"),
    ("cs.tax_report.pit38.base", "Základ daně, zaokrouhlený na celé zloté"),
    ("da.tax_report.pit38.base", "Skattegrundlag, afrundet til hele złoty"),
    ("de.tax_report.pit38.base", "Bemessungsgrundlage, auf volle Złoty gerundet"),
    ("el.tax_report.pit38.base", "Φορολογητέα βάση, στρογγυλοποιημένη σε ακέραια ζλότι"),
    ("en.tax_report.pit38.base", "Tax base, rounded to whole złoty"),
    ("es.tax_report.pit38.base", "Base imponible, redondeada al złoty entero"),
    ("fr.tax_report.pit38.base", "Base d’imposition, arrondie au złoty entier"),
    ("it.tax_report.pit38.base", "Base imponibile, arrotondata allo złoty intero"),
    ("nl.tax_report.pit38.base", "Heffingsgrondslag, afgerond op hele złoty"),
    ("pl.tax_report.pit38.base", "Podstawa obliczenia podatku (po zaokrągleniu do pełnych złotych)"),
    ("pt.tax_report.pit38.base", "Base tributável, arredondada ao złoty inteiro"),
    ("sk.tax_report.pit38.base", "Základ dane, zaokrúhlený na celé zloté"),
    ("sv.tax_report.pit38.base", "Skatteunderlag, avrundat till hela złoty"),
    ("bg.tax_report.pit38.tax", "Данък 19%, закръглен до цели злоти, преди приспадане на данък, платен в чужбина"),
    ("cs.tax_report.pit38.tax", "Daň 19 %, zaokrouhlená na celé zloté, před započtením daně zaplacené v zahraničí"),
    ("da.tax_report.pit38.tax", "Skat på 19 %, afrundet til hele złoty, før skat betalt i udlandet"),
    ("de.tax_report.pit38.tax", "Steuer zu 19 %, auf volle Złoty gerundet, vor Anrechnung im Ausland gezahlter Steuer"),
    ("el.tax_report.pit38.tax", "Φόρος 19%, στρογγυλοποιημένος σε ακέραια ζλότι, πριν από τυχόν φόρο που καταβλήθηκε στο εξωτερικό"),
    ("en.tax_report.pit38.tax", "Tax at 19%, rounded to whole złoty, before any tax paid abroad"),
    ("es.tax_report.pit38.tax", "Impuesto al 19 %, redondeado al złoty entero, antes de cualquier impuesto pagado en el extranjero"),
    ("fr.tax_report.pit38.tax", "Impôt à 19 %, arrondi au złoty entier, avant tout impôt payé à l’étranger"),
    ("it.tax_report.pit38.tax", "Imposta al 19%, arrotondata allo złoty intero, prima di eventuali imposte pagate all'estero"),
    ("nl.tax_report.pit38.tax", "Belasting van 19%, afgerond op hele złoty, vóór in het buitenland betaalde belasting"),
    ("pl.tax_report.pit38.tax", "Podatek 19%, po zaokrągleniu do pełnych złotych, przed odliczeniem podatku zapłaconego za granicą"),
    ("pt.tax_report.pit38.tax", "Imposto a 19%, arredondado ao złoty inteiro, antes de qualquer imposto pago no estrangeiro"),
    ("sk.tax_report.pit38.tax", "Daň 19 %, zaokrúhlená na celé zloté, pred započítaním dane zaplatenej v zahraničí"),
    ("sv.tax_report.pit38.tax", "Skatt på 19 %, avrundad till hela złoty, före skatt betald utomlands"),
    ("bg.tax_report.pit38.years_header", "Година по година, от първата година на синхронизираната ти история"),
    ("cs.tax_report.pit38.years_header", "Rok po roce, od prvního roku tvé synchronizované historie"),
    ("da.tax_report.pit38.years_header", "År for år, fra det første år i din synkroniserede historik"),
    ("de.tax_report.pit38.years_header", "Jahr für Jahr, ab dem ersten Jahr deiner synchronisierten Historie"),
    ("el.tax_report.pit38.years_header", "Έτος προς έτος, από το πρώτο έτος του συγχρονισμένου ιστορικού σου"),
    ("en.tax_report.pit38.years_header", "Year by year, from the first year of your synced history"),
    ("es.tax_report.pit38.years_header", "Año por año, desde el primer año de tu historial sincronizado"),
    ("fr.tax_report.pit38.years_header", "Année par année, depuis la première année de ton historique synchronisé"),
    ("it.tax_report.pit38.years_header", "Anno per anno, dal primo anno della tua cronologia sincronizzata"),
    ("nl.tax_report.pit38.years_header", "Jaar voor jaar, vanaf het eerste jaar van je gesynchroniseerde geschiedenis"),
    ("pl.tax_report.pit38.years_header", "Rok po roku, od pierwszego roku zsynchronizowanej historii"),
    ("pt.tax_report.pit38.years_header", "Ano a ano, desde o primeiro ano do teu histórico sincronizado"),
    ("sk.tax_report.pit38.years_header", "Rok po roku, od prvého roka tvojej synchronizovanej histórie"),
    ("sv.tax_report.pit38.years_header", "År för år, från det första året i din synkade historik"),
    ("bg.tax_report.pit38.carried_hint", "Колона d се получава само от синхронизираната ти история. Ако активи са купени по сметки, които не са синхронизирани тук, или предишната ти PIT-38 показва друга сума в колона f, използвай тази сума в колона d и преизчисли e и f."),
    ("cs.tax_report.pit38.carried_hint", "Sloupec d vychází jen z tvé synchronizované historie. Pokud byla aktiva nakoupena na účtech, které tu nejsou synchronizované, nebo pokud tvé předchozí PIT-38 uvádí ve sloupci f jinou částku, použij ve sloupci d tuto částku a přepočítej e a f."),
    ("da.tax_report.pit38.carried_hint", "Kolonne d bygger kun på din synkroniserede historik. Hvis du har købt aktiver på konti, der ikke er synkroniseret her, eller din forrige PIT-38 viser et andet beløb i kolonne f, så brug det beløb i kolonne d i stedet og beregn e og f igen."),
    ("de.tax_report.pit38.carried_hint", "Spalte d ergibt sich nur aus deiner synchronisierten Historie. Hast du Assets auf Konten gekauft, die hier nicht synchronisiert sind, oder weist deine letzte PIT-38 in Spalte f einen anderen Betrag aus, trage stattdessen diesen Betrag in Spalte d ein und berechne e und f neu."),
    ("el.tax_report.pit38.carried_hint", "Η στήλη d προκύπτει μόνο από το συγχρονισμένο ιστορικό σου. Αν αγοράστηκαν assets σε λογαριασμούς που δεν είναι συγχρονισμένοι εδώ ή αν η προηγούμενη PIT-38 σου δείχνει άλλο ποσό στη στήλη f, χρησιμοποίησε αυτό το ποσό στη στήλη d και υπολόγισε ξανά τα e και f."),
    ("en.tax_report.pit38.carried_hint", "Column d is built from your synced history only. If you bought assets on accounts not synced here, or your previous PIT-38 shows a different amount in column f, use that amount in column d instead and recompute e and f."),
    ("es.tax_report.pit38.carried_hint", "La columna d sale solo de tu historial sincronizado. Si compraste activos en cuentas no sincronizadas aquí, o tu anterior PIT-38 muestra otro importe en la columna f, usa ese importe en la columna d y vuelve a calcular e y f."),
    ("fr.tax_report.pit38.carried_hint", "La colonne d provient uniquement de ton historique synchronisé. Si tu as acheté des actifs sur des comptes non synchronisés ici, ou si ta précédente PIT-38 indique un autre montant en colonne f, reporte plutôt ce montant en colonne d et recalcule e et f."),
    ("it.tax_report.pit38.carried_hint", "La colonna d deriva solo dalla tua cronologia sincronizzata. Se hai acquistato asset su conti non sincronizzati qui, o se la tua precedente PIT-38 riporta un importo diverso nella colonna f, inserisci invece quell'importo nella colonna d e ricalcola e ed f."),
    ("nl.tax_report.pit38.carried_hint", "Kolom d volgt alleen uit je gesynchroniseerde geschiedenis. Heb je assets gekocht op accounts die hier niet gesynchroniseerd zijn, of vermeldt je vorige PIT-38 een ander bedrag in kolom f, vul dan dat bedrag in kolom d in en bereken e en f opnieuw."),
    ("pl.tax_report.pit38.carried_hint", "Kolumna d wynika wyłącznie z Twojej zsynchronizowanej historii. Jeśli aktywa były kupowane na kontach, które nie są tu zsynchronizowane, albo Twój poprzedni PIT-38 wykazuje w kolumnie f inną kwotę, wpisz w kolumnie d tę kwotę i przelicz e oraz f."),
    ("pt.tax_report.pit38.carried_hint", "A coluna d resulta apenas do teu histórico sincronizado. Se compraste ativos em contas não sincronizadas aqui, ou se a tua PIT-38 anterior indica outro montante na coluna f, usa esse montante na coluna d e recalcula e e f."),
    ("sk.tax_report.pit38.carried_hint", "Stĺpec d vychádza len z tvojej synchronizovanej histórie. Ak boli aktíva nakúpené na účtoch, ktoré tu nie sú synchronizované, alebo ak tvoje predchádzajúce PIT-38 uvádza v stĺpci f inú sumu, použi v stĺpci d túto sumu a prepočítaj e a f."),
    ("sv.tax_report.pit38.carried_hint", "Kolumn d bygger bara på din synkade historik. Om du köpt tillgångar på konton som inte är synkade här, eller om din föregående PIT-38 visar ett annat belopp i kolumn f, använd det beloppet i kolumn d i stället och räkna om e och f."),
    ("bg.tax_report.pit38.before_2019", "Разходи, направени преди 2019 г."),
    ("cs.tax_report.pit38.before_2019", "Náklady vynaložené před rokem 2019"),
    ("da.tax_report.pit38.before_2019", "Omkostninger afholdt før 2019"),
    ("de.tax_report.pit38.before_2019", "Vor 2019 entstandene Kosten"),
    ("el.tax_report.pit38.before_2019", "Κόστη που πραγματοποιήθηκαν πριν από το 2019"),
    ("en.tax_report.pit38.before_2019", "Costs incurred before 2019"),
    ("es.tax_report.pit38.before_2019", "Costes incurridos antes de 2019"),
    ("fr.tax_report.pit38.before_2019", "Coûts engagés avant 2019"),
    ("it.tax_report.pit38.before_2019", "Costi sostenuti prima del 2019"),
    ("nl.tax_report.pit38.before_2019", "Kosten gemaakt vóór 2019"),
    ("pl.tax_report.pit38.before_2019", "Koszty poniesione przed 2019 r."),
    ("pt.tax_report.pit38.before_2019", "Custos incorridos antes de 2019"),
    ("sk.tax_report.pit38.before_2019", "Náklady vynaložené pred rokom 2019"),
    ("sv.tax_report.pit38.before_2019", "Kostnader före 2019"),
    ("bg.tax_report.pit38.before_2019_hint", "Не са включени по-горе. Отчитат се само в PIT-38 ти за 2019 г. и само ако никога преди не са приспадани."),
    ("cs.tax_report.pit38.before_2019_hint", "Výše nezahrnuto. Počítají se jen v tvém PIT-38 za rok 2019, a jen pokud nikdy dříve nebyly odečteny."),
    ("da.tax_report.pit38.before_2019_hint", "Ikke medregnet ovenfor. De tæller kun i din PIT-38 for 2019, og kun hvis de aldrig er fratrukket før."),
    ("de.tax_report.pit38.before_2019_hint", "Oben nicht enthalten. Sie zählen nur in deiner PIT-38 für 2019, und nur wenn sie nie zuvor abgezogen wurden."),
    ("el.tax_report.pit38.before_2019_hint", "Δεν περιλαμβάνονται παραπάνω. Μετράνε μόνο στην PIT-38 σου για το 2019, και μόνο αν δεν έχουν αφαιρεθεί ποτέ πριν."),
    ("en.tax_report.pit38.before_2019_hint", "Not included above. They count only in your PIT-38 for 2019, and only if never deducted before."),
    ("es.tax_report.pit38.before_2019_hint", "No incluidos arriba. Solo cuentan en tu PIT-38 de 2019, y solo si nunca se dedujeron antes."),
    ("fr.tax_report.pit38.before_2019_hint", "Non inclus ci-dessus. Ils ne comptent que dans ta PIT-38 pour 2019, et seulement s’ils n’ont jamais été déduits auparavant."),
    ("it.tax_report.pit38.before_2019_hint", "Non inclusi sopra. Contano solo nella tua PIT-38 per il 2019, e solo se mai dedotti prima."),
    ("nl.tax_report.pit38.before_2019_hint", "Hierboven niet meegenomen. Ze tellen alleen in je PIT-38 over 2019, en alleen als ze nooit eerder zijn afgetrokken."),
    ("pl.tax_report.pit38.before_2019_hint", "Nieujęte powyżej. Uwzględnia się je tylko w PIT-38 za 2019 r. i tylko wtedy, gdy nie zostały wcześniej odliczone."),
    ("pt.tax_report.pit38.before_2019_hint", "Não incluídos acima. Só contam na tua PIT-38 de 2019, e só se nunca tiverem sido deduzidos antes."),
    ("sk.tax_report.pit38.before_2019_hint", "Vyššie nezahrnuté. Započítavajú sa len v tvojom PIT-38 za rok 2019, a len ak nikdy predtým neboli odpočítané."),
    ("sv.tax_report.pit38.before_2019_hint", "Inte medräknade ovan. De räknas bara i din PIT-38 för 2019, och bara om de aldrig dragits av tidigare."),
    ("bg.tax_report.warnings.rewards_not_revenue", "Награди, получени в активи (стейкинг, еърдропи, копаене, отдаване в заем), неотчетени нито като приход, нито като разход"),
    ("cs.tax_report.warnings.rewards_not_revenue", "Odměny přijaté v aktivech (staking, airdropy, těžba, půjčování), nezapočtené jako příjem ani náklad"),
    ("da.tax_report.warnings.rewards_not_revenue", "Belønninger modtaget i aktiver (staking, airdrops, mining, udlån), ikke medregnet som indtægt eller omkostning"),
    ("de.tax_report.warnings.rewards_not_revenue", "In Assets erhaltene Rewards (Staking, Airdrops, Mining, Verleihen), weder als Einnahme noch als Kosten angesetzt"),
    ("el.tax_report.warnings.rewards_not_revenue", "Ανταμοιβές που λήφθηκαν σε assets (staking, airdrops, mining, δανεισμός), που δεν μετρήθηκαν ούτε ως έσοδο ούτε ως κόστος"),
    ("en.tax_report.warnings.rewards_not_revenue", "Rewards received in assets (staking, airdrops, mining, lending), not counted as revenue or cost"),
    ("es.tax_report.warnings.rewards_not_revenue", "Recompensas recibidas en activos (staking, airdrops, minería, préstamos), no contadas como ingreso ni como coste"),
    ("fr.tax_report.warnings.rewards_not_revenue", "Récompenses reçues en actifs (staking, airdrops, minage, prêt), comptées ni comme recette ni comme coût"),
    ("it.tax_report.warnings.rewards_not_revenue", "Ricompense ricevute in asset (staking, airdrop, mining, prestito), non contate né come ricavo né come costo"),
    ("nl.tax_report.warnings.rewards_not_revenue", "In assets ontvangen beloningen (staking, airdrops, mining, uitlenen), niet meegeteld als opbrengst of kosten"),
    ("pl.tax_report.warnings.rewards_not_revenue", "Nagrody otrzymane w aktywach (staking, airdropy, kopanie, pożyczki), nieujęte jako przychód ani koszt"),
    ("pt.tax_report.warnings.rewards_not_revenue", "Recompensas recebidas em ativos (staking, airdrops, mineração, empréstimos), não contadas como receita nem como custo"),
    ("sk.tax_report.warnings.rewards_not_revenue", "Odmeny prijaté v aktívach (staking, airdropy, ťažba, požičiavanie), nezapočítané ako príjem ani náklad"),
    ("sv.tax_report.warnings.rewards_not_revenue", "Belöningar mottagna i tillgångar (staking, airdrops, mining, utlåning), inte räknade som intäkt eller kostnad"),
    ("bg.tax_report.warnings.rewards_not_revenue_hint", "Административните съдилища приемат, че наградите от стейкинг и еърдропите нямат разход и се облагат едва при продажба; данъчната администрация (KIS) ги третира като доход при получаване, деклариран в PIT-36. Копаенето няма разход (хардуерът и енергията не са разходи). Няма решение относно отдаването в заем. Стейбълкойните са виртуални валути; дали регулиран токен за електронни пари е такава, не е решено."),
    ("cs.tax_report.warnings.rewards_not_revenue_hint", "Správní soudy považují odměny ze stakingu a airdropy za bez nákladů a zdaněné až při prodeji; daňová správa (KIS) je považuje za příjem v okamžiku přijetí, uváděný v PIT-36. Těžba nemá náklad (hardware a energie nejsou náklady). Půjčování se netýká žádné rozhodnutí. Stablecoiny jsou virtuální měny; zda je jí regulovaný token elektronických peněz, nebylo rozhodnuto."),
    ("da.tax_report.warnings.rewards_not_revenue_hint", "Forvaltningsdomstolene behandler staking-belønninger og airdrops som uden omkostning og først beskattet ved salg; skattemyndigheden (KIS) behandler dem som indkomst ved modtagelsen, angivet på PIT-36. Mining har ingen omkostning (hardware og strøm er ikke omkostninger). Ingen afgørelse dækker udlån. Stablecoins er virtuelle valutaer; om en reguleret e-penge-token er det, er ikke afgjort."),
    ("de.tax_report.warnings.rewards_not_revenue_hint", "Verwaltungsgerichte behandeln Staking-Rewards und Airdrops als kostenlos und erst beim Verkauf steuerpflichtig; die Steuerverwaltung (KIS) behandelt sie als Einkünfte beim Erhalt, zu erklären in der PIT-36. Mining hat keine Kosten (Hardware und Strom sind keine Kosten). Zum Verleihen gibt es keine Entscheidung. Stablecoins sind virtuelle Währungen; ob ein regulierter E-Geld-Token eine ist, ist nicht entschieden."),
    ("el.tax_report.warnings.rewards_not_revenue_hint", "Τα διοικητικά δικαστήρια θεωρούν ότι οι ανταμοιβές staking και τα airdrops δεν έχουν κόστος και φορολογούνται μόνο κατά την πώληση· η φορολογική διοίκηση (KIS) τα αντιμετωπίζει ως εισόδημα κατά τη λήψη, που δηλώνεται στο PIT-36. Το mining δεν έχει κόστος (ο εξοπλισμός και η ενέργεια δεν είναι κόστη). Καμία απόφαση δεν καλύπτει τον δανεισμό. Τα stablecoins είναι εικονικά νομίσματα· αν ένα ρυθμιζόμενο token ηλεκτρονικού χρήματος είναι, δεν έχει κριθεί."),
    ("en.tax_report.warnings.rewards_not_revenue_hint", "Administrative courts treat staking rewards and airdrops as having no cost and taxed only when sold; the tax office (KIS) treats them as income when received, declared on PIT-36. Mining has no cost (hardware and power are not costs). No ruling covers lending. Stablecoins are virtual currencies; whether a regulated e-money token is one is undecided."),
    ("es.tax_report.warnings.rewards_not_revenue_hint", "Los tribunales administrativos consideran que las recompensas de staking y los airdrops no tienen coste y solo tributan al venderse; la administración tributaria (KIS) los trata como renta al recibirlos, declarada en el PIT-36. La minería no tiene coste (el hardware y la energía no son costes). Ninguna resolución trata los préstamos. Las stablecoins son monedas virtuales; no se ha decidido si un token de dinero electrónico regulado lo es."),
    ("fr.tax_report.warnings.rewards_not_revenue_hint", "Les tribunaux administratifs considèrent que les récompenses de staking et les airdrops n’ont aucun coût et ne sont imposés qu’à la cession ; l’administration fiscale (KIS) les traite comme un revenu à la réception, à déclarer sur la PIT-36. Le minage n’a aucun coût (le matériel et l’électricité ne sont pas des coûts). Aucune décision ne couvre le prêt. Les stablecoins sont des monnaies virtuelles ; la question de savoir si un jeton de monnaie électronique réglementé en est une n’est pas tranchée."),
    ("it.tax_report.warnings.rewards_not_revenue_hint", "I tribunali amministrativi considerano le ricompense di staking e gli airdrop privi di costo e tassati solo alla vendita; l'amministrazione fiscale (KIS) li tratta come reddito al ricevimento, da dichiarare nel PIT-36. Il mining non ha costo (hardware ed energia non sono costi). Nessuna decisione riguarda il prestito. Le stablecoin sono valute virtuali; se lo sia un token di moneta elettronica regolamentato non è stato deciso."),
    ("nl.tax_report.warnings.rewards_not_revenue_hint", "Bestuursrechters behandelen stakingbeloningen en airdrops als kosteloos en pas belast bij verkoop; de belastingdienst (KIS) behandelt ze als inkomen bij ontvangst, aan te geven op de PIT-36. Mining heeft geen kosten (hardware en stroom zijn geen kosten). Over uitlenen bestaat geen uitspraak. Stablecoins zijn virtuele valuta; of een gereguleerde e-geldtoken er een is, is niet beslist."),
    ("pl.tax_report.warnings.rewards_not_revenue_hint", "Sądy administracyjne uznają, że nagrody ze stakingu i airdropy nie mają kosztu i są opodatkowane dopiero przy zbyciu; organy podatkowe (KIS) traktują je jako przychód w chwili otrzymania, wykazywany w PIT-36. Kopanie nie ma kosztu (sprzęt i energia nie są kosztem). Pożyczek nie dotyczy żadne orzeczenie. Stablecoiny są walutami wirtualnymi; czy jest nią token będący pieniądzem elektronicznym, nie zostało rozstrzygnięte."),
    ("pt.tax_report.warnings.rewards_not_revenue_hint", "Os tribunais administrativos consideram que as recompensas de staking e os airdrops não têm custo e só são tributados na venda; a administração fiscal (KIS) trata-os como rendimento no momento da receção, declarado na PIT-36. A mineração não tem custo (o hardware e a energia não são custos). Nenhuma decisão abrange os empréstimos. As stablecoins são moedas virtuais; não está decidido se um token de moeda eletrónica regulado o é."),
    ("sk.tax_report.warnings.rewards_not_revenue_hint", "Správne súdy považujú odmeny zo stakingu a airdropy za bez nákladov a zdanené až pri predaji; daňová správa (KIS) ich považuje za príjem v okamihu prijatia, uvádzaný v PIT-36. Ťažba nemá náklad (hardvér a energia nie sú náklady). Požičiavania sa netýka žiadne rozhodnutie. Stablecoiny sú virtuálne meny; či je ňou regulovaný token elektronických peňazí, nebolo rozhodnuté."),
    ("sv.tax_report.warnings.rewards_not_revenue_hint", "Förvaltningsdomstolarna behandlar stakingbelöningar och airdrops som kostnadsfria och beskattade först vid försäljning; skattemyndigheten (KIS) behandlar dem som inkomst vid mottagandet, att deklarera i PIT-36. Mining har ingen kostnad (hårdvara och el är inga kostnader). Inget avgörande gäller utlåning. Stablecoins är virtuella valutor; om en reglerad e-penningtoken är det är inte avgjort."),
    ("bg.tax_report.warnings.unlinked_withdrawals_hint_pl", "Ако теглене е платило стоки или услуги или е погасило задължение, стойността му е приход. Свържи в Тракера всеки превод между собствените ти сметки."),
    ("cs.tax_report.warnings.unlinked_withdrawals_hint_pl", "Pokud výběr posloužil k zaplacení zboží nebo služeb nebo k vyrovnání dluhu, je jeho hodnota příjmem. Propoj v Trackeru každý převod mezi vlastními účty."),
    ("da.tax_report.warnings.unlinked_withdrawals_hint_pl", "Hvis en udbetaling betalte for varer eller tjenester eller indfriede en gæld, er dens værdi en indtægt. Knyt i Tracker hver overførsel mellem dine egne konti."),
    ("de.tax_report.warnings.unlinked_withdrawals_hint_pl", "Hat eine Auszahlung Waren oder Dienstleistungen bezahlt oder eine Schuld beglichen, ist ihr Wert eine Einnahme. Verknüpfe im Tracker jede Übertragung zwischen deinen eigenen Konten."),
    ("el.tax_report.warnings.unlinked_withdrawals_hint_pl", "Αν μια ανάληψη πλήρωσε αγαθά ή υπηρεσίες ή εξόφλησε χρέος, η αξία της είναι έσοδο. Σύνδεσε στο Tracker κάθε μεταφορά ανάμεσα στους δικούς σου λογαριασμούς."),
    ("en.tax_report.warnings.unlinked_withdrawals_hint_pl", "If a withdrawal paid for goods or services, or settled a debt, its value is revenue. Link each transfer between your own accounts in the Tracker."),
    ("es.tax_report.warnings.unlinked_withdrawals_hint_pl", "Si un retiro pagó bienes o servicios, o saldó una deuda, su valor es un ingreso. Vincula en el Tracker cada transferencia entre tus propias cuentas."),
    ("fr.tax_report.warnings.unlinked_withdrawals_hint_pl", "Si un retrait a payé des biens ou des services, ou réglé une dette, sa valeur est une recette. Associe dans le Tracker chaque transfert entre tes propres comptes."),
    ("it.tax_report.warnings.unlinked_withdrawals_hint_pl", "Se un prelievo ha pagato beni o servizi, o saldato un debito, il suo valore è un ricavo. Collega nel Tracker ogni trasferimento tra i tuoi conti."),
    ("nl.tax_report.warnings.unlinked_withdrawals_hint_pl", "Heeft een opname goederen of diensten betaald of een schuld voldaan, dan is de waarde ervan opbrengst. Koppel in de Tracker elke overboeking tussen je eigen accounts."),
    ("pl.tax_report.warnings.unlinked_withdrawals_hint_pl", "Jeśli wypłata posłużyła do zapłaty za towar lub usługę albo do uregulowania zobowiązania, jej wartość jest przychodem. Połącz w Trackerze każdy przelew między własnymi kontami."),
    ("pt.tax_report.warnings.unlinked_withdrawals_hint_pl", "Se um levantamento pagou bens ou serviços, ou saldou uma dívida, o seu valor é receita. Associa no Tracker cada transferência entre as tuas próprias contas."),
    ("sk.tax_report.warnings.unlinked_withdrawals_hint_pl", "Ak výber poslúžil na zaplatenie tovaru alebo služieb alebo na vyrovnanie dlhu, jeho hodnota je príjmom. Prepoj v Trackeri každý prevod medzi vlastnými účtami."),
    ("sv.tax_report.warnings.unlinked_withdrawals_hint_pl", "Om ett uttag betalade varor eller tjänster, eller reglerade en skuld, är dess värde en intäkt. Länka i Tracker varje överföring mellan dina egna konton."),
    ("bg.tax_report.warnings.deposit_costs_missing_hint", "Разходът за придобиването им не е включен в колона c. Ако са купени по сметка, която не е синхронизирана тук, добави този разход заедно с документа за него."),
    ("cs.tax_report.warnings.deposit_costs_missing_hint", "Jejich pořizovací náklad není ve sloupci c. Pokud byla nakoupena na účtu, který tu není synchronizovaný, připočti tento náklad i s dokladem."),
    ("da.tax_report.warnings.deposit_costs_missing_hint", "Deres anskaffelsesomkostning er ikke med i kolonne c. Hvis du købte dem på en konto, der ikke er synkroniseret her, så læg den omkostning til, med bilag."),
    ("de.tax_report.warnings.deposit_costs_missing_hint", "Ihre Anschaffungskosten sind nicht in Spalte c enthalten. Hast du sie auf einem Konto gekauft, das hier nicht synchronisiert ist, füge diese Kosten mit ihrem Beleg hinzu."),
    ("el.tax_report.warnings.deposit_costs_missing_hint", "Το κόστος απόκτησής τους δεν περιλαμβάνεται στη στήλη c. Αν αγοράστηκαν σε λογαριασμό που δεν είναι συγχρονισμένος εδώ, πρόσθεσε αυτό το κόστος, μαζί με το παραστατικό του."),
    ("en.tax_report.warnings.deposit_costs_missing_hint", "Their purchase cost is not in column c. If you bought them on an account not synced here, add that cost, with its document."),
    ("es.tax_report.warnings.deposit_costs_missing_hint", "Su coste de adquisición no está en la columna c. Si los compraste en una cuenta no sincronizada aquí, añade ese coste, con su justificante."),
    ("fr.tax_report.warnings.deposit_costs_missing_hint", "Leur coût d’acquisition n’est pas dans la colonne c. Si tu les as achetés sur un compte non synchronisé ici, ajoute ce coût, avec son justificatif."),
    ("it.tax_report.warnings.deposit_costs_missing_hint", "Il loro costo di acquisto non è nella colonna c. Se li hai acquistati su un conto non sincronizzato qui, aggiungi quel costo, con il relativo documento."),
    ("nl.tax_report.warnings.deposit_costs_missing_hint", "De aanschafkosten ervan staan niet in kolom c. Heb je ze gekocht op een account dat hier niet gesynchroniseerd is, tel die kosten dan op, met het bewijsstuk."),
    ("pl.tax_report.warnings.deposit_costs_missing_hint", "Koszt ich nabycia nie jest ujęty w kolumnie c. Jeśli zostały kupione na koncie, które nie jest tu zsynchronizowane, dolicz ten koszt wraz z dokumentem."),
    ("pt.tax_report.warnings.deposit_costs_missing_hint", "O seu custo de aquisição não está na coluna c. Se os compraste numa conta não sincronizada aqui, acrescenta esse custo, com o respetivo documento."),
    ("sk.tax_report.warnings.deposit_costs_missing_hint", "Ich obstarávací náklad nie je v stĺpci c. Ak boli nakúpené na účte, ktorý tu nie je synchronizovaný, pripočítaj tento náklad aj s dokladom."),
    ("sv.tax_report.warnings.deposit_costs_missing_hint", "Deras anskaffningskostnad finns inte i kolumn c. Om du köpte dem på ett konto som inte är synkat här, lägg till den kostnaden, med underlag."),
    ("bg.tax_report.warnings.unlinked_deposits", "Депозити, несвързани с теглене"),
    ("cs.tax_report.warnings.unlinked_deposits", "Vklady nepropojené s výběrem"),
    ("da.tax_report.warnings.unlinked_deposits", "Indbetalinger, der ikke er knyttet til en udbetaling"),
    ("de.tax_report.warnings.unlinked_deposits", "Einzahlungen, die mit keiner Auszahlung verknüpft sind"),
    ("el.tax_report.warnings.unlinked_deposits", "Καταθέσεις που δεν συνδέονται με ανάληψη"),
    ("en.tax_report.warnings.unlinked_deposits", "Deposits not linked to a withdrawal"),
    ("es.tax_report.warnings.unlinked_deposits", "Depósitos no vinculados a un retiro"),
    ("fr.tax_report.warnings.unlinked_deposits", "Dépôts non associés à un retrait"),
    ("it.tax_report.warnings.unlinked_deposits", "Depositi non collegati a un prelievo"),
    ("nl.tax_report.warnings.unlinked_deposits", "Stortingen die niet aan een opname gekoppeld zijn"),
    ("pl.tax_report.warnings.unlinked_deposits", "Wpłaty niepołączone z wypłatą"),
    ("pt.tax_report.warnings.unlinked_deposits", "Depósitos não associados a um levantamento"),
    ("sk.tax_report.warnings.unlinked_deposits", "Vklady neprepojené s výberom"),
    ("sv.tax_report.warnings.unlinked_deposits", "Insättningar som inte är länkade till ett uttag"),
    ("bg.tax_report.warnings.missing_nbp_rates", "Суми без курс на NBP (таблица A) от предходните дни, отчетени по-горе като 0"),
    ("cs.tax_report.warnings.missing_nbp_rates", "Částky bez kurzu NBP (tabulka A) z předchozích dnů, výše započtené jako 0"),
    ("da.tax_report.warnings.missing_nbp_rates", "Beløb uden NBP-kurs (tabel A) fra dagene før, medregnet som 0 ovenfor"),
    ("de.tax_report.warnings.missing_nbp_rates", "Werte ohne NBP-Kurs (Tabelle A) aus den Tagen davor, oben mit 0 angesetzt"),
    ("el.tax_report.warnings.missing_nbp_rates", "Ποσά χωρίς ισοτιμία NBP (πίνακας A) από τις προηγούμενες ημέρες, που μετρήθηκαν παραπάνω ως 0"),
    ("en.tax_report.warnings.missing_nbp_rates", "Values with no NBP table A rate from the days before them, counted as 0 above"),
    ("es.tax_report.warnings.missing_nbp_rates", "Importes sin tipo NBP (tabla A) de los días anteriores, contados como 0 arriba"),
    ("fr.tax_report.warnings.missing_nbp_rates", "Montants sans cours NBP (tableau A) des jours qui les précèdent, comptés 0 ci-dessus"),
    ("it.tax_report.warnings.missing_nbp_rates", "Importi senza cambio NBP (tabella A) dei giorni precedenti, contati come 0 sopra"),
    ("nl.tax_report.warnings.missing_nbp_rates", "Bedragen zonder NBP-koers (tabel A) uit de dagen ervoor, hierboven als 0 meegeteld"),
    ("pl.tax_report.warnings.missing_nbp_rates", "Wartości bez kursu średniego NBP (tabela A) z dni je poprzedzających, ujęte powyżej jako 0"),
    ("pt.tax_report.warnings.missing_nbp_rates", "Montantes sem taxa NBP (tabela A) dos dias anteriores, contados como 0 acima"),
    ("sk.tax_report.warnings.missing_nbp_rates", "Sumy bez kurzu NBP (tabuľka A) z predchádzajúcich dní, vyššie započítané ako 0"),
    ("sv.tax_report.warnings.missing_nbp_rates", "Belopp utan NBP-kurs (tabell A) från dagarna före, räknade som 0 ovan"),
    ("bg.tax_report.warnings.missing_nbp_rates_hint", "Всеки ред: валута, ден. Генерирай отчета отново, когато курсовете на NBP могат да бъдат изтеглени, или преизчисли тези суми по курса от последната таблица на NBP преди всеки ден."),
    ("cs.tax_report.warnings.missing_nbp_rates_hint", "Každý řádek: měna, den. Vygeneruj přehled znovu, až bude možné kurzy NBP načíst, nebo tyto částky přepočti kurzem z poslední tabulky NBP před každým dnem."),
    ("da.tax_report.warnings.missing_nbp_rates_hint", "Hver linje: valuta, dag. Generér rapporten igen, når NBP's kurser kan hentes, eller omregn disse beløb til kursen i den seneste NBP-tabel før hver dag."),
    ("de.tax_report.warnings.missing_nbp_rates_hint", "Jede Zeile: Währung, Tag. Erstelle den Bericht erneut, sobald die NBP-Kurse abrufbar sind, oder rechne diese Beträge zum Kurs der letzten NBP-Tabelle vor dem jeweiligen Tag um."),
    ("el.tax_report.warnings.missing_nbp_rates_hint", "Κάθε γραμμή: νόμισμα, ημέρα. Δημιούργησε ξανά την αναφορά όταν οι ισοτιμίες της NBP μπορούν να ληφθούν, ή μετάτρεψε αυτά τα ποσά με την ισοτιμία του τελευταίου πίνακα της NBP πριν από κάθε ημέρα."),
    ("en.tax_report.warnings.missing_nbp_rates_hint", "Each line: currency, day. Generate the report again once NBP's rates can be fetched, or convert these at the rate of the last NBP table before each day."),
    ("es.tax_report.warnings.missing_nbp_rates_hint", "Cada línea: moneda, día. Vuelve a generar el informe cuando se puedan obtener los tipos del NBP, o convierte estos importes al tipo de la última tabla del NBP anterior a cada día."),
    ("fr.tax_report.warnings.missing_nbp_rates_hint", "Chaque ligne : devise, jour. Génère à nouveau le rapport quand les cours NBP pourront être récupérés, ou convertis ces montants au cours du dernier tableau NBP précédant chaque jour."),
    ("it.tax_report.warnings.missing_nbp_rates_hint", "Ogni riga: valuta, giorno. Genera di nuovo il report quando i cambi NBP saranno disponibili, oppure converti questi importi al cambio dell'ultima tabella NBP precedente a ciascun giorno."),
    ("nl.tax_report.warnings.missing_nbp_rates_hint", "Elke regel: valuta, dag. Genereer het rapport opnieuw zodra de NBP-koersen op te halen zijn, of reken deze bedragen om tegen de koers van de laatste NBP-tabel vóór elke dag."),
    ("pl.tax_report.warnings.missing_nbp_rates_hint", "Każdy wiersz: waluta, dzień. Wygeneruj raport ponownie, gdy kursy NBP będą dostępne, albo przelicz te kwoty po kursie z ostatniej tabeli NBP sprzed każdego dnia."),
    ("pt.tax_report.warnings.missing_nbp_rates_hint", "Cada linha: moeda, dia. Gera o relatório novamente quando as taxas do NBP puderem ser obtidas, ou converte estes montantes à taxa da última tabela do NBP anterior a cada dia."),
    ("sk.tax_report.warnings.missing_nbp_rates_hint", "Každý riadok: mena, deň. Vygeneruj prehľad znova, keď bude možné kurzy NBP načítať, alebo tieto sumy prepočítaj kurzom z poslednej tabuľky NBP pred každým dňom."),
    ("sv.tax_report.warnings.missing_nbp_rates_hint", "Varje rad: valuta, dag. Skapa rapporten igen när NBP:s kurser kan hämtas, eller räkna om dessa belopp till kursen i den senaste NBP-tabellen före varje dag."),
    ("bg.tax_report.warnings.coin_fees_hint_pl", "Не са включени по-горе: такса, записана на отделен ред, не посочва сделка. Такса, платена в актив, погасява задължение във виртуална валута: стойността ѝ е приход, а също и разход, освен ако е събрана при замяна на една виртуална валута срещу друга."),
    ("cs.tax_report.warnings.coin_fees_hint_pl", "Výše nezahrnuto: poplatek zaúčtovaný na samostatném řádku neoznačuje žádný obchod. Poplatek zaplacený v aktivu vyrovnává závazek virtuální měnou: jeho hodnota je příjmem a také nákladem, pokud nebyl účtován při směně jedné virtuální měny za jinou."),
    ("da.tax_report.warnings.coin_fees_hint_pl", "Ikke medregnet ovenfor: et gebyr bogført på sin egen linje angiver ingen handel. Et gebyr betalt i et aktiv indfrier en forpligtelse i virtuel valuta: dets værdi er en indtægt, og også en omkostning, medmindre det blev opkrævet ved en ombytning af én virtuel valuta til en anden."),
    ("de.tax_report.warnings.coin_fees_hint_pl", "Oben nicht enthalten: eine als eigene Zeile gebuchte Gebühr nennt keinen Trade. Eine in einem Asset gezahlte Gebühr begleicht eine Verbindlichkeit in virtueller Währung: ihr Wert ist eine Einnahme und auch Kosten, außer sie fiel beim Tausch einer virtuellen Währung gegen eine andere an."),
    ("el.tax_report.warnings.coin_fees_hint_pl", "Δεν περιλαμβάνονται παραπάνω: μια χρέωση καταχωρισμένη σε δική της γραμμή δεν αντιστοιχεί σε συναλλαγή. Μια χρέωση που πληρώθηκε σε asset εξοφλεί υποχρέωση σε εικονικό νόμισμα: η αξία της είναι έσοδο, και επίσης κόστος, εκτός αν χρεώθηκε σε ανταλλαγή ενός εικονικού νομίσματος με άλλο."),
    ("en.tax_report.warnings.coin_fees_hint_pl", "Not counted above: a fee booked as a row of its own names no trade. A fee paid in an asset settles an obligation in virtual currency: its value is revenue, and also a cost unless it was charged on an exchange of one virtual currency for another."),
    ("es.tax_report.warnings.coin_fees_hint_pl", "No incluidas arriba: una comisión registrada en una fila propia no indica ninguna operación. Una comisión pagada en un activo salda una obligación en moneda virtual: su valor es un ingreso, y también un coste salvo que se cobrara en un intercambio de una moneda virtual por otra."),
    ("fr.tax_report.warnings.coin_fees_hint_pl", "Non inclus ci-dessus : des frais inscrits sur une ligne à part ne désignent aucune transaction. Des frais payés en actif règlent une obligation en monnaie virtuelle : leur valeur est une recette, et aussi un coût sauf s’ils ont été prélevés sur un échange d’une monnaie virtuelle contre une autre."),
    ("it.tax_report.warnings.coin_fees_hint_pl", "Non incluse sopra: una commissione registrata come riga a sé non indica alcuna operazione. Una commissione pagata in un asset estingue un'obbligazione in valuta virtuale: il suo valore è un ricavo, e anche un costo salvo che sia stata addebitata su uno scambio tra valute virtuali."),
    ("nl.tax_report.warnings.coin_fees_hint_pl", "Hierboven niet meegeteld: een vergoeding die als eigen regel geboekt is, noemt geen trade. Een vergoeding betaald in een asset voldoet een verplichting in virtuele valuta: de waarde ervan is opbrengst, en ook kosten, tenzij ze werd gerekend bij een ruil van de ene virtuele valuta tegen een andere."),
    ("pl.tax_report.warnings.coin_fees_hint_pl", "Nieujęte powyżej: opłata zapisana jako osobny wiersz nie wskazuje transakcji. Opłata zapłacona w aktywach reguluje zobowiązanie walutą wirtualną: jej wartość jest przychodem, a także kosztem, chyba że pobrano ją przy wymianie jednej waluty wirtualnej na inną."),
    ("pt.tax_report.warnings.coin_fees_hint_pl", "Não incluídas acima: uma comissão registada numa linha própria não indica nenhuma operação. Uma comissão paga num ativo liquida uma obrigação em moeda virtual: o seu valor é receita, e também custo, salvo se tiver sido cobrada numa troca de uma moeda virtual por outra."),
    ("sk.tax_report.warnings.coin_fees_hint_pl", "Vyššie nezahrnuté: poplatok zaúčtovaný na samostatnom riadku neoznačuje žiadny obchod. Poplatok zaplatený v aktíve vyrovnáva záväzok virtuálnou menou: jeho hodnota je príjmom a aj nákladom, pokiaľ nebol účtovaný pri výmene jednej virtuálnej meny za inú."),
    ("sv.tax_report.warnings.coin_fees_hint_pl", "Inte medräknade ovan: en avgift bokförd på en egen rad anger ingen affär. En avgift betald i en tillgång reglerar en skuld i virtuell valuta: dess värde är en intäkt, och även en kostnad om den inte togs ut vid ett byte av en virtuell valuta mot en annan."),
];

pub enum Arg<'a> {
    /// Plain text: escaped when the key is an HTML key.
    Text(&'a str),
    /// Markup that is already safe (`html_safe` in Rails): never escaped.
    Html(&'a str),
    /// `count:`; it also selects the plural form.
    Count(i64),
}

fn leaf(full_key: &str) -> Option<&'static str> {
    TRANSLATIONS.binary_search_by(|(k, _)| (*k).cmp(full_key)).ok().map(|i| TRANSLATIONS[i].1)
        .or_else(|| FROM_GEMS.iter().find(|(k, _)| *k == full_key).map(|(_, v)| *v))
}

/// config/locales/plurals.rb: only `ru` has a rule; every other locale uses I18n's one/other.
fn category(locale: &str, n: i64) -> &'static str {
    if n == 1 {
        "one"
    } else if locale == "ru" && [2, 3, 4].contains(&(n % 10)) && ![12, 13, 14, 22, 23, 24].contains(&(n % 100)) {
        "few"
    } else {
        "other"
    }
}

fn in_locale(locale: &str, key: &str, count: Option<i64>) -> Option<&'static str> {
    let base = format!("{locale}.{key}");
    if let Some(text) = leaf(&base) {
        return Some(text);
    }
    let n = count?;
    let form = |name: &str| leaf(&format!("{base}.{name}"));
    if n == 0 {
        if let Some(zero) = form("zero") {
            return Some(zero);
        }
    }
    // I18n::Backend::Pluralization: with a rule, the explicit "0"/"1" keys win and a missing form
    // falls back to `other`. Without one Rails raises on a missing form; here it reads as missing.
    if locale == "ru" && (n == 0 || n == 1) {
        if let Some(explicit) = form(if n == 0 { "0" } else { "1" }) {
            return Some(explicit);
        }
    }
    form(category(locale, n)).or_else(|| if locale == "ru" { form("other") } else { None })
}

fn lookup(locale: &str, key: &str, count: Option<i64>) -> Option<&'static str> {
    in_locale(locale, key, count).or_else(|| if locale == DEFAULT { None } else { in_locale(DEFAULT, key, count) })
}

/// ERB::Util.html_escape.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// I18n.interpolate: `%{name}` takes the argument, `%%{name}` is a literal `%{name}`. Rails raises
/// on a name with no argument; here the placeholder stays as written.
fn interpolate(template: &str, args: &[(&str, Arg)], escape_text: bool) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("%{") {
        let Some(length) = rest[start..].find('}') else { break };
        let name = &rest[start + 2..start + length];
        if rest[..start].ends_with('%') {
            out.push_str(&rest[..start - 1]);
            out.push_str(&rest[start..=start + length]);
        } else {
            out.push_str(&rest[..start]);
            match args.iter().find(|(n, _)| *n == name) {
                Some((_, Arg::Text(value))) if escape_text => out.push_str(&escape(value)),
                Some((_, Arg::Text(value))) | Some((_, Arg::Html(value))) => out.push_str(value),
                Some((_, Arg::Count(n))) => out.push_str(&n.to_string()),
                None => out.push_str(&rest[start..=start + length]),
            }
        }
        rest = &rest[start + length + 1..];
    }
    out.push_str(rest);
    out
}

fn count_of(args: &[(&str, Arg)]) -> Option<i64> {
    args.iter().find_map(|(name, arg)| match arg {
        Arg::Count(n) if *name == "count" => Some(*n),
        _ => None,
    })
}

/// ActiveSupport::HtmlSafeTranslation: a key ending in `_html`, or whose last segment is `html`.
fn html_key(key: &str) -> bool {
    key.ends_with("_html") || key == "html" || key.ends_with(".html")
}

/// String#titleize for a key segment: `two_factor_title` -> `Two Factor Title`.
fn titleize(segment: &str) -> String {
    let words = segment.trim_start_matches('_');
    let words = words.strip_suffix("_id").unwrap_or(words).replace('_', " ");
    let mut out = String::with_capacity(words.len());
    let mut word_start = true;
    for c in words.chars() {
        if word_start { out.extend(c.to_uppercase()) } else { out.push(c) }
        word_start = !c.is_alphanumeric();
    }
    out
}

fn value_text(arg: &Arg) -> String {
    match arg {
        Arg::Text(v) | Arg::Html(v) => v.to_string(),
        Arg::Count(n) => n.to_string(),
    }
}

/// The view helper `t`: markup ready to place in a page. A plain key's text is escaped; an HTML
/// key's text is trusted and only its plain arguments are escaped; a missing key is Rails'
/// `translation_missing` span.
pub fn t(locale: &str, key: &str, args: &[(&str, Arg)]) -> String {
    match lookup(locale, key, count_of(args)) {
        Some(template) if html_key(key) => interpolate(template, args, true),
        Some(template) => escape(&interpolate(template, args, false)),
        None => {
            let mut title = format!("translation missing: {locale}.{key}");
            for (name, arg) in args {
                title.push_str(&format!(", {name}: {}", escape(&value_text(arg))));
            }
            let last = key.rsplit('.').next().unwrap_or(key);
            format!("<span class=\"translation_missing\" title=\"{}\">{}</span>", escape(&title), escape(&titleize(last)))
        }
    }
}

/// `I18n.t`: plain text, for flash messages and plain-text responses. A missing key reads
/// "Translation missing: <locale>.<key>", as I18n's default handler returns.
pub fn text(locale: &str, key: &str, args: &[(&str, Arg)]) -> String {
    match lookup(locale, key, count_of(args)) {
        Some(template) => interpolate(template, args, false),
        None => format!("Translation missing: {locale}.{key}"),
    }
}

/// `I18n.t(keys[0], default: keys[1..])` with symbol defaults, as ActiveModel looks up an error message or an
/// attribute's name: each locale of the fallback chain (this one, then English) tries every key in turn before the
/// next locale is tried. `None` when no key has a text in either.
pub fn text_chain(locale: &str, keys: &[String], args: &[(&str, Arg)]) -> Option<String> {
    let count = count_of(args);
    let locales: &[&str] = if locale == DEFAULT { &[DEFAULT] } else { &[locale, DEFAULT] };
    locales.iter().find_map(|l| keys.iter().find_map(|key| in_locale(l, key, count))).map(|template| interpolate(template, args, false))
}

/// Every embedded (full key, text) pair, for the test that compares the table with Rails' own.
pub fn all() -> &'static [(&'static str, &'static str)] {
    TRANSLATIONS
}
