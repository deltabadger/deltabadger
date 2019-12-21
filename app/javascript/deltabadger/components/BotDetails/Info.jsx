import React from 'react'

export const Info = ({ bot }) => (
  <div className="tab-pane pl-3 pr-3" id="info" role="tabpanel" aria-labelledby="info-tab">
    <p className="mt-2"><b>Exchanges</b></p>
    <p>Deltabadger works with external exchanges. At the moment, we support <a href="https://bitbay.net" target="_blank" title="Bitbay">Bitbay</a> and <a href="https://kraken.com" target="_blank" title="Kraken">Kraken</a>. We recommend Kraken as a reputable exchange that has never been hacked. Use Bitbay if you want to make your purchases with PLN.</p>
    <p>However, don't keep your coins at any exchange too long. Login to your account and do regularly withdraws at least once a month.</p>
    <p>We will add more exchanges in the future. Let us know if you are interested in a particular one.</p>
    <p className="mt-2"><b>"Smart intervals"</b></p>
    <p>You may be surprised that Deltabadger buys amounts different from defined.</p>
    <p>In fact, it keeps your desired ratio using the smallest purchases allowed by the exchange. In that way, you not only get the best avaraging, but also it protect bot from being stopped when the price of Bitcoin goes "too high" so your defined amount becomes smaller than the minimal transaction allowed by the exchange.</p>
    <p><b>Example:</b> Your defined schedule is $1/hour. The smallest transaction size allowed is 0.002BTC, and the current Bitcoin price is $5000. That means that the exchange will not allow transactions lower than $10 (0.002*$5000). The bot will buy BTC worth $10 and schedule the next purchase in 10 hours. On average, you will get your desired  $1/hour ratio.</p>
  </div>
)
