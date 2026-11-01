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
