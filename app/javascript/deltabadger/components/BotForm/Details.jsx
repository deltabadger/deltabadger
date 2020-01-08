import React from 'react'

export const Details = ({ bot }) => {
  return (
    <div className="db-bots__item db-bot-data">
      <ul className="nav nav-tabs" id="botFormInfo" role="tablist">
        <li className="nav-item">
          <a className="nav-link active" id="botFormInfoTab" data-toggle="tab" href="#botFormInfoTab"  role="tab" aria-controls="botFormInfoTab"  aria-selected="false">Info</a>
        </li>
      </ul>
      <div className="tab-content" id="botFormInfo">
        <div className="tab-pane show active pl-3 pr-3" id="botFormInfoTab" role="tabpanel" aria-labelledby="botFormInfoTab">
          <p className="mt-2"><b>Exchanges</b></p>
          <p>Deltabadger works with cryptoexchanges. At the moment, we support <a href="https://bitbay.net" target="_blank" rel="noopener" title="Bitbay">Bitbay</a> and <a href="https://r.kraken.com/deltabadger" target="_blank" rel="noopener" title="Kraken">Kraken</a>. We recommend Kraken as a reputable exchange that has never been hacked. Use Bitbay if you want to make your purchases with PLN.</p>
          <p>However, don't keep your coins at any exchange too long. Login to your account and do regularly withdraws at least once a month.</p>
          <p>We will add more exchanges in the future. Let us know if you are interested in a particular one.</p>
          <p className="mt-2"><b>"Smart intervals"</b></p>
          <p>You may be surprised that the bot schedule is different from the one you defined.</p>
          <p>In fact, it keeps your desired ratio using the smallest purchases allowed by the exchange. In that way, you not only get the best avaraging but also it protects bot from being stopped when the price of Bitcoin goes "too high," so your defined amount becomes smaller than the minimal transaction allowed by the exchange.</p>
          <p><b>Example:</b> Your defined schedule is $1/hour. The smallest transaction size allowed is 0.002BTC, and the current Bitcoin price is $5000. That means that the exchange will not allow transactions lower than $10 (0.002*$5000). The bot will buy BTC worth $10 and schedule the next purchase in 10 hours. On average, you will get your desired  $1/hour ratio.</p>
          <p className="mt-2"><b>Links</b></p>
          <ul className="mb-5">
            <li><a href="https://www.youtube.com/watch?v=dltaIrhUUvY" target="_blank" rel="noopener">DCA for Beginners (video)</a></li>
            <li><a href="https://medium.com/predict/the-power-of-dollar-cost-averaging-into-bitcoin-2fad7fb12ce6" target="_blank" rel="noopener">The power of DCA into Bitcoin</a></li>
            <li><a href="https://dcabtc.com/" target="_blank" rel="noopener">DCA Simulator</a></li>
          </ul>
        </div>
      </div>
    </div>
  )
}
