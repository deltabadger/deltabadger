import React from 'react'

export const Details = ({ bot }) => {
  return (
    <div className="db-bots__item db-bots__item--data">
      <ul className="nav nav-tabs" id="botFormInfo" role="tablist">
        <li className="nav-item">
          <a className="nav-link active" id="botFormInfoTab" data-toggle="tab" href="#botFormInfoTab"  role="tab" aria-controls="botFormInfoTab"  aria-selected="false">Info</a>
        </li>
      </ul>
      <div className="tab-content" id="botFormInfo">
        <div className="tab-pane show active pl-4 pr-4" id="botFormInfoTab" role="tabpanel" aria-labelledby="botFormInfoTab">
          <div className="db-showif db-showif--pick-exchange">
            <p className="mt-2"><b>Exchanges</b></p>
            <p>Deltabadger works with cryptoexchanges. At the moment, we support <a href="https://www.binance.com/en/register?ref=NUYVIP6R" target="_blank" rel="nofollow">Binance</a>, <a href="https://www.binance.us" target="_blank" rel="nofollow">Binance.US</a>, <a href="https://auth.bitbay.net/ref/Hhb7ZrAv2GrA" target="_blank" rel="nofollow" title="Bitbay">Bitbay</a>, <a href="https://pro.coinbase.com/" target="_blank" rel="nofollow">Coinbase Pro</a>, <a href="https://exchange.gemini.com" target="_blank" rel="nofollow">Gemini</a> and <a href="https://r.kraken.com/deltabadger" target="_blank" rel="nofollow" title="Kraken">Kraken</a>. We recommend Kraken as a reputable exchange that has never been hacked. Use Bitbay if you want to make your purchases in PLN.</p>
            <p>To make your bot useful, first, you need to verify your exchange account and transfer some funds there.</p>
            <p>However, don't keep your coins at any exchange too long. Login to your account and do regularly withdraws at least once a month.</p>
            <p>We will add more exchanges in the future. Let us know if you are interested in a particular one.</p>
          </div>
          <div className="db-showif db-showif--setup">
            <p className="mt-2"><b>"Smart intervals"</b></p>
            <p>When smart intervals are turned on, Deltabadger executes your desired schedule in the smallest transactions allowed by the exchange. Otherwise, they'll be used only if necessary.</p>
            <p className="mt-2"><b>Why?</b></p>
            <p>There are two benefits:</p>
            <p>The more frequent schedule allows you to experience the benefits of DCA quicker and smooths it out even more.</p>
            <p>Also, sometimes smart intervals are necessary. Every exchange has a minimum transaction size limit. Some exchanges define it in BTC, so the USD value changes over time. If the amount you want to buy is smaller, Deltabadger keeps your desired ratio using the minimum allowed size but adjusting the time interval. It protects bot from being stopped when your defined amount falls below the threshold.</p>
            <p className="mt-2"><b>Example:</b></p>
            <p>Let's say the current exchange limit is $20. If you set the transaction size for $24/day, the bot will do exactly that. However, if you define it as $1/hour, it will mimic the ratio using the allowed size, which equals $20 every 20 hours. On average, it results in the same ratio ($24/day = $1/hour = $20/20hours).</p>
          </div>
          <p className="mt-2"><b>New to dollar-cost averaging?</b></p>
          <p>Those links will help:</p>
          <ul className="mb-5">
            <li><a href="https://www.youtube.com/watch?v=dltaIrhUUvY" target="_blank" rel="nofollow">DCA for Beginners (video)</a></li>
            <li><a href="https://medium.com/predict/the-power-of-dollar-cost-averaging-into-bitcoin-2fad7fb12ce6" target="_blank" rel="nofollow">The power of DCA into Bitcoin</a></li>
            <li><a href="https://dcabtc.com/" target="_blank" rel="nofollow">DCA Simulator</a></li>
          </ul>
        </div>
      </div>
    </div>
  )
}
