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
    // A12-2b: Austria's NOTE blocks (D-TAX-AT-REWARDS, -AVERAGE, -INCOME-1-MARCH-2022, -MONTHLY-VALUE, -SPECULATION), in every
    // report locale; the AT report renders in German.
    ("bg.tax_report.warnings.zero_cost_rewards", "Награди от стейкинг и еърдропи, водени с цена на придобиване нула"),
    ("bg.tax_report.warnings.zero_cost_rewards_hint", "Австрия ги облага едва при продажбата им, върху цялата им стойност (§ 27b Abs. 2 EStG); при получаването им те не са доход. Ако дадена награда всъщност е била лихва за отдаване на активите ти назаем под друго име, тя е доход при получаването: декларирай я като такъв."),
    ("bg.tax_report.warnings.average_cost", "Цената на придобиване на активи, придобити от 1 март 2021 г., е от 2023 г. плъзгащата се средна цена за актив и сметка (§ 27a Abs. 4 Z 3a EStG)"),
    ("bg.tax_report.warnings.average_cost_hint", "Показаната дата на придобиване при такава продажба е най-ранната от осреднените единици. Ако в сметката има и активи, придобити преди 1 март 2021 г., те се смятат за продадени първи, освен ако не си избрал друго (§ 3 KryptowährungsVO)."),
    ("bg.tax_report.warnings.income_before_regime", "Доходи, получени преди 1 март 2022 г., които не са част от доходите, облагани с 27,5%"),
    ("bg.tax_report.warnings.income_before_regime_hint", "Преди 1 март 2022 г. доходи като майнинг или отдаване назаем се облагаха по действащите тогава правила, а не по § 27b EStG. Декларирай ги така, както изискват тези правила."),
    ("bg.tax_report.warnings.monthly_value", "Доходи, изплатени повече от три пъти в месеца, оценени по цената при затваряне на 1-во число на този месец (§ 4 Abs. 2 KryptowährungsVO)"),
    ("bg.tax_report.warnings.monthly_value_hint", "Всеки ред: сметка, актив, вид, месец, плащания. Правилото важи за всяко споразумение поотделно; ако две споразумения в една сметка са ти платили всяко не повече от три пъти, оцени всяко плащане по цената на собствения му ден."),
    ("bg.tax_report.warnings.speculation", "Продажби преди 1 март 2022 г., облагани като спекулативни сделки (държани една година или по-малко, § 31 EStG)"),
    ("bg.tax_report.warnings.speculation_hint", "Те се компенсират помежду си в рамките на годината, но не и с печалбите и загубите, облагани с 27,5%. Сборът им е освободен от данък, ако не надвишава 440 € за годината; общата загуба не се приспада от нищо друго."),
    ("cs.tax_report.warnings.zero_cost_rewards", "Odměny ze stakingu a airdropy, vedené s pořizovací cenou nula"),
    ("cs.tax_report.warnings.zero_cost_rewards_hint", "Rakousko je daní až při prodeji, z celé jejich hodnoty (§ 27b Abs. 2 EStG); při obdržení nejsou příjmem. Pokud byla odměna ve skutečnosti úrokem za půjčení tvých aktiv pod jiným názvem, je příjmem při obdržení: přiznej ji tak."),
    ("cs.tax_report.warnings.average_cost", "Pořizovací cena aktiv pořízených od 1. března 2021 je od roku 2023 klouzavý průměr za aktivum a účet (§ 27a Abs. 4 Z 3a EStG)"),
    ("cs.tax_report.warnings.average_cost_hint", "Uvedené datum pořízení u takového prodeje je nejdřívější ze zprůměrovaných jednotek. Pokud účet drží i aktiva pořízená před 1. březnem 2021, považují se za prodaná jako první, pokud sis nezvolil jinak (§ 3 KryptowährungsVO)."),
    ("cs.tax_report.warnings.income_before_regime", "Příjmy obdržené před 1. březnem 2022, které nejsou součástí příjmů zdaněných sazbou 27,5 %"),
    ("cs.tax_report.warnings.income_before_regime_hint", "Před 1. březnem 2022 se příjmy jako těžba nebo půjčování zdaňovaly podle tehdy platných pravidel, ne podle § 27b EStG. Přiznej je tak, jak tato pravidla vyžadují."),
    ("cs.tax_report.warnings.monthly_value", "Příjmy vyplacené více než třikrát za měsíc, oceněné závěrečnou cenou 1. dne daného měsíce (§ 4 Abs. 2 KryptowährungsVO)"),
    ("cs.tax_report.warnings.monthly_value_hint", "Každý řádek: účet, aktivum, druh, měsíc, platby. Pravidlo platí pro každé ujednání zvlášť; pokud ti dvě ujednání na jednom účtu zaplatila každé nejvýše třikrát, oceň místo toho každou platbu cenou jejího dne."),
    ("cs.tax_report.warnings.speculation", "Prodeje před 1. březnem 2022 zdaněné jako spekulační obchody (držené rok nebo kratší dobu, § 31 EStG)"),
    ("cs.tax_report.warnings.speculation_hint", "Vzájemně se započítávají v rámci roku, ale ne se zisky a ztrátami zdaněnými sazbou 27,5 %. Jejich součet je osvobozen od daně, pokud za rok nepřesáhne 440 €; celková ztráta se s ničím jiným nezapočítává."),
    ("da.tax_report.warnings.zero_cost_rewards", "Staking-belønninger og airdrops, opført med en anskaffelsespris på nul"),
    ("da.tax_report.warnings.zero_cost_rewards_hint", "Østrig beskatter dem først, når de sælges, af hele deres værdi (§ 27b Abs. 2 EStG); de er ikke indkomst ved modtagelsen. Hvis en belønning i virkeligheden var rente for udlån af dine aktiver under et andet navn, er den indkomst ved modtagelsen: angiv den som sådan."),
    ("da.tax_report.warnings.average_cost", "Anskaffelsesprisen for aktiver anskaffet fra 1. marts 2021 er fra 2023 det glidende gennemsnit pr. aktiv og konto (§ 27a Abs. 4 Z 3a EStG)"),
    ("da.tax_report.warnings.average_cost_hint", "Den viste anskaffelsesdato for et sådant salg er den tidligste af de gennemsnitsberegnede enheder. Hvis en konto også har aktiver anskaffet før 1. marts 2021, anses de for solgt først, medmindre du har valgt andet (§ 3 KryptowährungsVO)."),
    ("da.tax_report.warnings.income_before_regime", "Indkomst modtaget før 1. marts 2022, ikke en del af indkomsten beskattet med 27,5 %"),
    ("da.tax_report.warnings.income_before_regime_hint", "Før 1. marts 2022 blev indkomst som mining eller udlån beskattet efter de dagældende regler, ikke § 27b EStG. Angiv den, som disse regler kræver."),
    ("da.tax_report.warnings.monthly_value", "Indkomst udbetalt mere end tre gange på en måned, værdiansat til lukkekursen den 1. i måneden (§ 4 Abs. 2 KryptowährungsVO)"),
    ("da.tax_report.warnings.monthly_value_hint", "Hver linje: konto, aktiv, type, måned, udbetalinger. Reglen gælder pr. ordning; hvis to ordninger på én konto hver har betalt dig tre gange eller færre, så værdiansæt i stedet hver udbetaling til kursen på dens egen dag."),
    ("da.tax_report.warnings.speculation", "Salg før 1. marts 2022 beskattet som spekulation (ejet et år eller mindre, § 31 EStG)"),
    ("da.tax_report.warnings.speculation_hint", "De modregnes indbyrdes inden for året, men ikke med gevinster og tab beskattet med 27,5 %. Deres samlede beløb er skattefrit, hvis det ikke overstiger 440 € i året; et samlet tab kan ikke modregnes i andet."),
    ("de.tax_report.warnings.zero_cost_rewards", "Staking-Rewards und Airdrops, mit Anschaffungskosten von null angesetzt"),
    ("de.tax_report.warnings.zero_cost_rewards_hint", "Österreich besteuert diese erst beim Verkauf, mit ihrem vollen Wert (§ 27b Abs. 2 EStG); beim Erhalt sind sie keine Einkünfte. War ein Reward in Wahrheit ein Entgelt für das Verleihen deiner Assets unter anderem Namen, ist er beim Zufluss steuerpflichtig: gib ihn so an."),
    ("de.tax_report.warnings.average_cost", "Die Anschaffungskosten von ab 1. März 2021 angeschafften Assets sind ab 2023 der gleitende Durchschnittspreis je Asset und Konto (§ 27a Abs. 4 Z 3a EStG)"),
    ("de.tax_report.warnings.average_cost_hint", "Das angezeigte Anschaffungsdatum eines solchen Verkaufs ist das früheste der gemittelten Einheiten. Hält ein Konto auch vor dem 1. März 2021 angeschaffte Assets, gelten diese als zuerst verkauft, sofern du nichts anderes gewählt hast (§ 3 KryptowährungsVO)."),
    ("de.tax_report.warnings.income_before_regime", "Vor dem 1. März 2022 zugeflossene Einkünfte, nicht Teil der zu 27,5 % besteuerten Einkünfte"),
    ("de.tax_report.warnings.income_before_regime_hint", "Vor dem 1. März 2022 wurden Einkünfte wie Mining oder Verleihen nach den damals geltenden Regeln besteuert, nicht nach § 27b EStG. Gib sie so an, wie diese Regeln es verlangen."),
    ("de.tax_report.warnings.monthly_value", "Mehr als dreimal im Monat zugeflossene Einkünfte, bewertet mit dem Tagesendkurs am Monatsersten (§ 4 Abs. 2 KryptowährungsVO)"),
    ("de.tax_report.warnings.monthly_value_hint", "Jede Zeile: Konto, Asset, Art, Monat, Zuflüsse. Die Regel gilt je Vorgang; haben dir zwei Vorgänge auf einem Konto jeweils höchstens dreimal gezahlt, bewerte jede Zahlung stattdessen mit dem Kurs ihres Tages."),
    ("de.tax_report.warnings.speculation", "Veräußerungen vor dem 1. März 2022, als Spekulationsgeschäfte steuerpflichtig (Behaltedauer höchstens ein Jahr, § 31 EStG)"),
    ("de.tax_report.warnings.speculation_hint", "Sie werden innerhalb des Jahres miteinander ausgeglichen, nicht aber mit den zu 27,5 % besteuerten Gewinnen und Verlusten. Ihre Summe ist steuerfrei, wenn sie im Jahr 440 € nicht übersteigt; ein Gesamtverlust ist nicht ausgleichsfähig."),
    ("el.tax_report.warnings.zero_cost_rewards", "Ανταμοιβές staking και airdrops, με κόστος κτήσης μηδέν"),
    ("el.tax_report.warnings.zero_cost_rewards_hint", "Η Αυστρία τα φορολογεί μόνο όταν πωληθούν, επί ολόκληρης της αξίας τους (§ 27b Abs. 2 EStG)· κατά τη λήψη τους δεν αποτελούν εισόδημα. Αν μια ανταμοιβή ήταν στην πραγματικότητα τόκος για τον δανεισμό των assets σου με άλλο όνομα, αποτελεί εισόδημα κατά τη λήψη: δήλωσέ την ως τέτοιο."),
    ("el.tax_report.warnings.average_cost", "Το κόστος κτήσης των assets που αποκτήθηκαν από την 1η Μαρτίου 2021 είναι από το 2023 ο κινητός μέσος όρος ανά asset και λογαριασμό (§ 27a Abs. 4 Z 3a EStG)"),
    ("el.tax_report.warnings.average_cost_hint", "Η ημερομηνία κτήσης που εμφανίζεται για μια τέτοια πώληση είναι η παλαιότερη από τις μονάδες του μέσου όρου. Αν ένας λογαριασμός περιέχει και assets που αποκτήθηκαν πριν από την 1η Μαρτίου 2021, θεωρείται ότι πωλούνται πρώτα, εκτός αν επέλεξες διαφορετικά (§ 3 KryptowährungsVO)."),
    ("el.tax_report.warnings.income_before_regime", "Εισόδημα που ελήφθη πριν από την 1η Μαρτίου 2022, εκτός του εισοδήματος που φορολογείται με 27,5%"),
    ("el.tax_report.warnings.income_before_regime_hint", "Πριν από την 1η Μαρτίου 2022, εισόδημα όπως από mining ή δανεισμό φορολογούνταν με τους τότε ισχύοντες κανόνες, όχι με το § 27b EStG. Δήλωσέ το όπως απαιτούν αυτοί οι κανόνες."),
    ("el.tax_report.warnings.monthly_value", "Εισόδημα που καταβλήθηκε περισσότερες από τρεις φορές σε έναν μήνα, αποτιμημένο στην τιμή κλεισίματος της 1ης του μήνα (§ 4 Abs. 2 KryptowährungsVO)"),
    ("el.tax_report.warnings.monthly_value_hint", "Κάθε γραμμή: λογαριασμός, asset, είδος, μήνας, πληρωμές. Ο κανόνας ισχύει ανά συμφωνία· αν δύο συμφωνίες σε έναν λογαριασμό σε πλήρωσαν η καθεμία τρεις φορές ή λιγότερες, αποτίμησε αντί γι' αυτό κάθε πληρωμή στην τιμή της δικής της ημέρας."),
    ("el.tax_report.warnings.speculation", "Πωλήσεις πριν από την 1η Μαρτίου 2022 που φορολογούνται ως κερδοσκοπικές (διακράτηση έως ενός έτους, § 31 EStG)"),
    ("el.tax_report.warnings.speculation_hint", "Συμψηφίζονται μεταξύ τους μέσα στο έτος, αλλά όχι με κέρδη και ζημίες που φορολογούνται με 27,5%. Το σύνολό τους είναι αφορολόγητο αν δεν υπερβαίνει τα 440 € στο έτος· μια συνολική ζημία δεν συμψηφίζεται με τίποτε άλλο."),
    ("en.tax_report.warnings.zero_cost_rewards", "Staking rewards and airdrops, held at a cost of zero"),
    ("en.tax_report.warnings.zero_cost_rewards_hint", "Austria taxes these only when they are sold, on their whole value (§ 27b Abs. 2 EStG); they are not income when received. If a reward was in fact interest for lending your assets under another name, it is income when received: declare it as such."),
    ("en.tax_report.warnings.average_cost", "Cost of assets acquired from 1 March 2021 is the moving average per asset and account from 2023 (§ 27a Abs. 4 Z 3a EStG)"),
    ("en.tax_report.warnings.average_cost_hint", "The acquisition date shown for such a sale is the earliest of the units averaged. Where an account also holds assets acquired before 1 March 2021, those count as sold first, unless you chose otherwise (§ 3 KryptowährungsVO)."),
    ("en.tax_report.warnings.income_before_regime", "Income received before 1 March 2022, not part of the income taxed at 27.5%"),
    ("en.tax_report.warnings.income_before_regime_hint", "Before 1 March 2022, income such as mining or lending was taxed under the rules then in force, not § 27b EStG. Declare it as those rules require."),
    ("en.tax_report.warnings.monthly_value", "Income paid more than three times in a month, valued at the closing price of the 1st of that month (§ 4 Abs. 2 KryptowährungsVO)"),
    ("en.tax_report.warnings.monthly_value_hint", "Each line: account, asset, type, month, payments. The rule applies per arrangement; if two arrangements on one account paid you three times or fewer each, value each payment at its own day's price instead."),
    ("en.tax_report.warnings.speculation", "Sales before 1 March 2022 taxed as speculation (held a year or less, § 31 EStG)"),
    ("en.tax_report.warnings.speculation_hint", "They net with each other within the year, but not with gains and losses taxed at 27.5%. Their total is tax-free if it is no more than €440 in the year; an overall loss offsets nothing else."),
    ("es.tax_report.warnings.zero_cost_rewards", "Recompensas de staking y airdrops, con un coste de adquisición de cero"),
    ("es.tax_report.warnings.zero_cost_rewards_hint", "Austria solo los grava cuando se venden, por su valor íntegro (§ 27b Abs. 2 EStG); al recibirlos no son renta. Si una recompensa era en realidad un interés por prestar tus activos con otro nombre, es renta al recibirla: decláralo como tal."),
    ("es.tax_report.warnings.average_cost", "El coste de adquisición de los activos adquiridos desde el 1 de marzo de 2021 es, desde 2023, la media móvil por activo y cuenta (§ 27a Abs. 4 Z 3a EStG)"),
    ("es.tax_report.warnings.average_cost_hint", "La fecha de adquisición indicada en una venta así es la más antigua de las unidades promediadas. Si una cuenta también tiene activos adquiridos antes del 1 de marzo de 2021, se consideran vendidos primero, salvo que hayas elegido otra cosa (§ 3 KryptowährungsVO)."),
    ("es.tax_report.warnings.income_before_regime", "Rentas recibidas antes del 1 de marzo de 2022, no incluidas en las rentas que tributan al 27,5 %"),
    ("es.tax_report.warnings.income_before_regime_hint", "Antes del 1 de marzo de 2022, rentas como la minería o el préstamo tributaban según las normas entonces vigentes, no según el § 27b EStG. Decláralas como esas normas exigen."),
    ("es.tax_report.warnings.monthly_value", "Rentas pagadas más de tres veces en un mes, valoradas al precio de cierre del día 1 de ese mes (§ 4 Abs. 2 KryptowährungsVO)"),
    ("es.tax_report.warnings.monthly_value_hint", "Cada línea: cuenta, activo, tipo, mes, pagos. La regla se aplica por acuerdo; si dos acuerdos en una misma cuenta te pagaron tres veces o menos cada uno, valora en su lugar cada pago al precio de su propio día."),
    ("es.tax_report.warnings.speculation", "Ventas antes del 1 de marzo de 2022 gravadas como especulación (mantenidas un año o menos, § 31 EStG)"),
    ("es.tax_report.warnings.speculation_hint", "Se compensan entre sí dentro del año, pero no con las ganancias y pérdidas que tributan al 27,5 %. Su total está exento si no supera 440 € en el año; una pérdida total no compensa nada más."),
    ("fr.tax_report.warnings.zero_cost_rewards", "Récompenses de staking et airdrops, comptés à un coût d’acquisition nul"),
    ("fr.tax_report.warnings.zero_cost_rewards_hint", "L’Autriche ne les impose qu’à leur vente, sur leur valeur entière (§ 27b Abs. 2 EStG) ; à leur réception, ils ne sont pas un revenu. Si une récompense était en réalité un intérêt pour le prêt de tes actifs sous un autre nom, c’est un revenu à sa réception : déclare-la comme tel."),
    ("fr.tax_report.warnings.average_cost", "Le coût d’acquisition des actifs acquis à partir du 1er mars 2021 est, depuis 2023, la moyenne mobile par actif et par compte (§ 27a Abs. 4 Z 3a EStG)"),
    ("fr.tax_report.warnings.average_cost_hint", "La date d’acquisition indiquée pour une telle vente est la plus ancienne des unités moyennées. Si un compte détient aussi des actifs acquis avant le 1er mars 2021, ils sont réputés vendus en premier, sauf choix contraire de ta part (§ 3 KryptowährungsVO)."),
    ("fr.tax_report.warnings.income_before_regime", "Revenus perçus avant le 1er mars 2022, hors des revenus imposés à 27,5 %"),
    ("fr.tax_report.warnings.income_before_regime_hint", "Avant le 1er mars 2022, les revenus tels que le minage ou le prêt étaient imposés selon les règles alors en vigueur, non selon le § 27b EStG. Déclare-les comme ces règles l’exigent."),
    ("fr.tax_report.warnings.monthly_value", "Revenus versés plus de trois fois dans un mois, évalués au cours de clôture du 1er de ce mois (§ 4 Abs. 2 KryptowährungsVO)"),
    ("fr.tax_report.warnings.monthly_value_hint", "Chaque ligne : compte, actif, type, mois, versements. La règle s’applique par dispositif ; si deux dispositifs sur un même compte t’ont versé chacun trois fois ou moins, évalue plutôt chaque versement au cours de son propre jour."),
    ("fr.tax_report.warnings.speculation", "Ventes avant le 1er mars 2022 imposées comme spéculation (détention d’un an au plus, § 31 EStG)"),
    ("fr.tax_report.warnings.speculation_hint", "Elles se compensent entre elles dans l’année, mais pas avec les gains et pertes imposés à 27,5 %. Leur total est exonéré s’il ne dépasse pas 440 € dans l’année ; une perte globale ne s’impute sur rien d’autre."),
    ("it.tax_report.warnings.zero_cost_rewards", "Ricompense di staking e airdrop, con un costo di acquisto pari a zero"),
    ("it.tax_report.warnings.zero_cost_rewards_hint", "L'Austria li tassa solo alla vendita, sull'intero valore (§ 27b Abs. 2 EStG); al ricevimento non sono reddito. Se una ricompensa era in realtà un interesse per il prestito dei tuoi asset sotto un altro nome, è reddito al ricevimento: dichiarala come tale."),
    ("it.tax_report.warnings.average_cost", "Il costo di acquisto degli asset acquistati dal 1° marzo 2021 è, dal 2023, la media mobile per asset e per conto (§ 27a Abs. 4 Z 3a EStG)"),
    ("it.tax_report.warnings.average_cost_hint", "La data di acquisto indicata per una vendita di questo tipo è la più vecchia tra le unità mediate. Se un conto detiene anche asset acquistati prima del 1° marzo 2021, questi si considerano venduti per primi, salvo tua scelta diversa (§ 3 KryptowährungsVO)."),
    ("it.tax_report.warnings.income_before_regime", "Redditi percepiti prima del 1° marzo 2022, non compresi nei redditi tassati al 27,5%"),
    ("it.tax_report.warnings.income_before_regime_hint", "Prima del 1° marzo 2022, redditi come il mining o il prestito erano tassati secondo le regole allora in vigore, non secondo il § 27b EStG. Dichiarali come quelle regole richiedono."),
    ("it.tax_report.warnings.monthly_value", "Redditi corrisposti più di tre volte in un mese, valutati al prezzo di chiusura del 1° di quel mese (§ 4 Abs. 2 KryptowährungsVO)"),
    ("it.tax_report.warnings.monthly_value_hint", "Ogni riga: conto, asset, tipo, mese, pagamenti. La regola si applica per singolo accordo; se due accordi sullo stesso conto ti hanno pagato ciascuno tre volte o meno, valuta invece ogni pagamento al prezzo del suo giorno."),
    ("it.tax_report.warnings.speculation", "Vendite prima del 1° marzo 2022 tassate come speculazione (detenzione di un anno o meno, § 31 EStG)"),
    ("it.tax_report.warnings.speculation_hint", "Si compensano tra loro nell'anno, ma non con le plusvalenze e minusvalenze tassate al 27,5%. Il loro totale è esente se non supera 440 € nell'anno; una perdita complessiva non compensa nient'altro."),
    ("nl.tax_report.warnings.zero_cost_rewards", "Stakingbeloningen en airdrops, met een aanschafprijs van nul"),
    ("nl.tax_report.warnings.zero_cost_rewards_hint", "Oostenrijk belast ze pas bij verkoop, over hun volledige waarde (§ 27b Abs. 2 EStG); bij ontvangst zijn ze geen inkomen. Was een beloning in werkelijkheid rente voor het uitlenen van je assets onder een andere naam, dan is ze inkomen bij ontvangst: geef haar zo aan."),
    ("nl.tax_report.warnings.average_cost", "De aanschafprijs van assets verkregen vanaf 1 maart 2021 is vanaf 2023 het voortschrijdend gemiddelde per asset en account (§ 27a Abs. 4 Z 3a EStG)"),
    ("nl.tax_report.warnings.average_cost_hint", "De getoonde aanschafdatum van zo'n verkoop is de vroegste van de gemiddelde eenheden. Houdt een account ook assets aan die vóór 1 maart 2021 zijn verkregen, dan gelden die als eerst verkocht, tenzij je anders hebt gekozen (§ 3 KryptowährungsVO)."),
    ("nl.tax_report.warnings.income_before_regime", "Inkomsten ontvangen vóór 1 maart 2022, geen deel van de inkomsten belast tegen 27,5%"),
    ("nl.tax_report.warnings.income_before_regime_hint", "Vóór 1 maart 2022 werden inkomsten zoals mining of uitlenen belast volgens de toen geldende regels, niet volgens § 27b EStG. Geef ze aan zoals die regels vereisen."),
    ("nl.tax_report.warnings.monthly_value", "Inkomsten die meer dan drie keer in een maand zijn uitbetaald, gewaardeerd tegen de slotkoers van de 1e van die maand (§ 4 Abs. 2 KryptowährungsVO)"),
    ("nl.tax_report.warnings.monthly_value_hint", "Elke regel: account, asset, soort, maand, uitbetalingen. De regel geldt per regeling; hebben twee regelingen op één account je elk drie keer of minder betaald, waardeer dan elke uitbetaling tegen de koers van haar eigen dag."),
    ("nl.tax_report.warnings.speculation", "Verkopen vóór 1 maart 2022 belast als speculatie (een jaar of korter aangehouden, § 31 EStG)"),
    ("nl.tax_report.warnings.speculation_hint", "Ze worden binnen het jaar met elkaar verrekend, maar niet met winsten en verliezen die tegen 27,5% worden belast. Hun totaal is belastingvrij als het in het jaar niet meer dan € 440 bedraagt; een totaalverlies is met niets anders verrekenbaar."),
    ("pl.tax_report.warnings.zero_cost_rewards", "Nagrody ze stakingu i airdropy, ujęte z kosztem nabycia równym zero"),
    ("pl.tax_report.warnings.zero_cost_rewards_hint", "Austria opodatkowuje je dopiero przy sprzedaży, od całej ich wartości (§ 27b Abs. 2 EStG); w chwili otrzymania nie są dochodem. Jeśli nagroda była w rzeczywistości odsetkami za pożyczenie Twoich aktywów pod inną nazwą, jest dochodem w chwili otrzymania: wykaż ją jako taki."),
    ("pl.tax_report.warnings.average_cost", "Koszt nabycia aktywów nabytych od 1 marca 2021 r. to od 2023 r. średnia krocząca dla danego aktywa i konta (§ 27a Abs. 4 Z 3a EStG)"),
    ("pl.tax_report.warnings.average_cost_hint", "Data nabycia podana przy takiej sprzedaży to najwcześniejsza z uśrednionych jednostek. Jeśli na koncie są też aktywa nabyte przed 1 marca 2021 r., uznaje się je za sprzedane jako pierwsze, chyba że wybrano inaczej (§ 3 KryptowährungsVO)."),
    ("pl.tax_report.warnings.income_before_regime", "Dochody otrzymane przed 1 marca 2022 r., nienależące do dochodów opodatkowanych stawką 27,5%"),
    ("pl.tax_report.warnings.income_before_regime_hint", "Przed 1 marca 2022 r. dochody takie jak kopanie czy pożyczanie były opodatkowane według ówczesnych przepisów, a nie § 27b EStG. Wykaż je tak, jak wymagają te przepisy."),
    ("pl.tax_report.warnings.monthly_value", "Dochody wypłacone więcej niż trzy razy w miesiącu, wycenione po cenie zamknięcia z 1. dnia tego miesiąca (§ 4 Abs. 2 KryptowährungsVO)"),
    ("pl.tax_report.warnings.monthly_value_hint", "Każdy wiersz: konto, aktywo, rodzaj, miesiąc, wypłaty. Reguła dotyczy każdego ustalenia osobno; jeśli dwa ustalenia na jednym koncie wypłaciły Ci każde najwyżej trzy razy, wyceń zamiast tego każdą wypłatę po cenie z jej dnia."),
    ("pl.tax_report.warnings.speculation", "Sprzedaże przed 1 marca 2022 r. opodatkowane jako spekulacyjne (posiadanie przez rok lub krócej, § 31 EStG)"),
    ("pl.tax_report.warnings.speculation_hint", "Kompensują się wzajemnie w ciągu roku, ale nie z zyskami i stratami opodatkowanymi stawką 27,5%. Ich suma jest zwolniona z podatku, jeśli w roku nie przekracza 440 €; łączna strata nie pomniejsza niczego innego."),
    ("pt.tax_report.warnings.zero_cost_rewards", "Recompensas de staking e airdrops, com um custo de aquisição de zero"),
    ("pt.tax_report.warnings.zero_cost_rewards_hint", "A Áustria só os tributa quando são vendidos, sobre o seu valor total (§ 27b Abs. 2 EStG); ao serem recebidos não são rendimento. Se uma recompensa era na verdade um juro pelo empréstimo dos teus ativos com outro nome, é rendimento quando recebida: declara-a como tal."),
    ("pt.tax_report.warnings.average_cost", "O custo de aquisição dos ativos adquiridos a partir de 1 de março de 2021 é, desde 2023, a média móvel por ativo e conta (§ 27a Abs. 4 Z 3a EStG)"),
    ("pt.tax_report.warnings.average_cost_hint", "A data de aquisição indicada numa venda destas é a mais antiga das unidades da média. Se uma conta também detém ativos adquiridos antes de 1 de março de 2021, consideram-se vendidos primeiro, salvo se escolheste outra coisa (§ 3 KryptowährungsVO)."),
    ("pt.tax_report.warnings.income_before_regime", "Rendimentos recebidos antes de 1 de março de 2022, fora dos rendimentos tributados a 27,5%"),
    ("pt.tax_report.warnings.income_before_regime_hint", "Antes de 1 de março de 2022, rendimentos como mineração ou empréstimo eram tributados pelas regras então em vigor, não pelo § 27b EStG. Declara-os como essas regras exigem."),
    ("pt.tax_report.warnings.monthly_value", "Rendimentos pagos mais de três vezes num mês, avaliados ao preço de fecho do dia 1 desse mês (§ 4 Abs. 2 KryptowährungsVO)"),
    ("pt.tax_report.warnings.monthly_value_hint", "Cada linha: conta, ativo, tipo, mês, pagamentos. A regra aplica-se por acordo; se dois acordos numa mesma conta te pagaram cada um três vezes ou menos, avalia antes cada pagamento ao preço do seu próprio dia."),
    ("pt.tax_report.warnings.speculation", "Vendas antes de 1 de março de 2022 tributadas como especulação (detidos um ano ou menos, § 31 EStG)"),
    ("pt.tax_report.warnings.speculation_hint", "Compensam-se entre si dentro do ano, mas não com ganhos e perdas tributados a 27,5%. O seu total está isento se não exceder 440 € no ano; uma perda global não compensa mais nada."),
    ("sk.tax_report.warnings.zero_cost_rewards", "Odmeny zo stakingu a airdropy, vedené s obstarávacou cenou nula"),
    ("sk.tax_report.warnings.zero_cost_rewards_hint", "Rakúsko ich zdaňuje až pri predaji, z celej ich hodnoty (§ 27b Abs. 2 EStG); pri prijatí nie sú príjmom. Ak bola odmena v skutočnosti úrokom za požičanie tvojich aktív pod iným názvom, je príjmom pri prijatí: priznaj ju tak."),
    ("sk.tax_report.warnings.average_cost", "Obstarávacia cena aktív obstaraných od 1. marca 2021 je od roku 2023 kĺzavý priemer za aktívum a účet (§ 27a Abs. 4 Z 3a EStG)"),
    ("sk.tax_report.warnings.average_cost_hint", "Uvedený dátum obstarania pri takom predaji je najskorší zo spriemerovaných jednotiek. Ak účet drží aj aktíva obstarané pred 1. marcom 2021, považujú sa za predané ako prvé, ak si nezvolil inak (§ 3 KryptowährungsVO)."),
    ("sk.tax_report.warnings.income_before_regime", "Príjmy prijaté pred 1. marcom 2022, ktoré nie sú súčasťou príjmov zdaňovaných sadzbou 27,5 %"),
    ("sk.tax_report.warnings.income_before_regime_hint", "Pred 1. marcom 2022 sa príjmy ako ťažba alebo požičiavanie zdaňovali podľa vtedy platných pravidiel, nie podľa § 27b EStG. Priznaj ich tak, ako tieto pravidlá vyžadujú."),
    ("sk.tax_report.warnings.monthly_value", "Príjmy vyplatené viac ako trikrát za mesiac, ocenené uzatváracou cenou 1. dňa daného mesiaca (§ 4 Abs. 2 KryptowährungsVO)"),
    ("sk.tax_report.warnings.monthly_value_hint", "Každý riadok: účet, aktívum, druh, mesiac, platby. Pravidlo platí pre každé dojednanie zvlášť; ak ti dve dojednania na jednom účte zaplatili každé najviac trikrát, oceň namiesto toho každú platbu cenou jej dňa."),
    ("sk.tax_report.warnings.speculation", "Predaje pred 1. marcom 2022 zdanené ako špekulačné obchody (držané rok alebo kratšie, § 31 EStG)"),
    ("sk.tax_report.warnings.speculation_hint", "Navzájom sa započítavajú v rámci roka, ale nie so ziskami a stratami zdaňovanými sadzbou 27,5 %. Ich súčet je oslobodený od dane, ak za rok nepresiahne 440 €; celková strata sa s ničím iným nezapočítava."),
    ("sv.tax_report.warnings.zero_cost_rewards", "Stakingbelöningar och airdrops, upptagna till ett omkostnadsbelopp på noll"),
    ("sv.tax_report.warnings.zero_cost_rewards_hint", "Österrike beskattar dem först när de säljs, på hela deras värde (§ 27b Abs. 2 EStG); när de tas emot är de inte inkomst. Om en belöning i själva verket var ränta för utlåning av dina tillgångar under ett annat namn är den inkomst när den tas emot: deklarera den som sådan."),
    ("sv.tax_report.warnings.average_cost", "Omkostnadsbeloppet för tillgångar förvärvade från den 1 mars 2021 är från 2023 det glidande genomsnittet per tillgång och konto (§ 27a Abs. 4 Z 3a EStG)"),
    ("sv.tax_report.warnings.average_cost_hint", "Förvärvsdatumet som visas för en sådan försäljning är det tidigaste av de genomsnittsberäknade enheterna. Om ett konto även har tillgångar förvärvade före den 1 mars 2021 räknas de som sålda först, om du inte har valt annat (§ 3 KryptowährungsVO)."),
    ("sv.tax_report.warnings.income_before_regime", "Inkomster mottagna före den 1 mars 2022, inte en del av inkomsterna som beskattas med 27,5 %"),
    ("sv.tax_report.warnings.income_before_regime_hint", "Före den 1 mars 2022 beskattades inkomster som mining eller utlåning enligt de regler som gällde då, inte § 27b EStG. Deklarera dem som de reglerna kräver."),
    ("sv.tax_report.warnings.monthly_value", "Inkomster som betalats ut mer än tre gånger under en månad, värderade till stängningskursen den 1:a i månaden (§ 4 Abs. 2 KryptowährungsVO)"),
    ("sv.tax_report.warnings.monthly_value_hint", "Varje rad: konto, tillgång, typ, månad, utbetalningar. Regeln gäller per arrangemang; om två arrangemang på ett konto har betalat dig tre gånger eller färre vardera, värdera i stället varje utbetalning till kursen på dess egen dag."),
    ("sv.tax_report.warnings.speculation", "Försäljningar före den 1 mars 2022 beskattade som spekulation (innehav i ett år eller kortare, § 31 EStG)"),
    ("sv.tax_report.warnings.speculation_hint", "De kvittas mot varandra inom året, men inte mot vinster och förluster som beskattas med 27,5 %. Summan är skattefri om den inte överstiger 440 € under året; en total förlust kan inte kvittas mot något annat."),
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
