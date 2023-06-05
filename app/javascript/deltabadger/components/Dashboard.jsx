import React, {useEffect, useState} from 'react'
import { connect } from 'react-redux';
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { TradingBot } from './TradingBot'
import { isEmpty } from '../utils/array'
import {
  startBot,
  openBot,
  closeAllBots,
  loadBots
} from '../bot_actions'
import API from "../lib/API";
import { WithdrawalBot } from "./WithdrawalBot";
import { WebhookBot } from "./WebhookBot";

let apiKeyTimeout;

const DashboardTemplate = ({
  isHodler,
  isLegendaryBadger,
  bots = [],
  numberOfPages = 0,
  errors = {},
  currentBot,
  startBot,
  openBot,
  closeAllBots,
  loadBots
}) => {

  const [exchanges, setExchanges] = useState([]);
  const [page, setPage] = useState(1);
  const [step, setStep] = useState(0);

  useEffect(() => {
    closeAllBots()
    const shouldOpenFirstBot = step === 0
    loadBots(shouldOpenFirstBot, page)
  }, [page,])

  const fetchExchanges = (type) => {
    API.getExchanges(type).then(data => setExchanges(data.data))
  }

  useEffect( () => {
    fetchExchanges('trading')
  }, [])

  useEffect( () => {
    if( bots.length === 0 && page !== 1){
      setPage(page-1)
    }
  },[bots])

  const reloadPage = () => {
    loadBots(false,page)
  }

  const buildBotsList = (botsToRender, b) => {
    if(b.bot_type === 'free') {
      botsToRender.push(
        <TradingBot
          showLimitOrders={isHodler || isLegendaryBadger}
          key={`${b.id}-${b.id == currentBot}`}
          bot={b}
          open={currentBot && (b.id == currentBot.id)}
          errors={errors[b.id]}
          fetchExchanges={() => fetchExchanges('trading')}
          exchanges={exchanges}
          apiKeyTimeout={apiKeyTimeout}
          reloadPage={reloadPage}
        />
      )
    } else if(b.bot_type === 'withdrawal') {
      botsToRender.push(
        <WithdrawalBot
          showLimitOrders={isHodler || isLegendaryBadger}
          key={`${b.id}-${b.id == currentBot}`}
          bot={b}
          open={currentBot && (b.id == currentBot.id)}
          errors={errors[b.id]}
          fetchExchanges={() => fetchExchanges('withdrawal')}
          exchanges={exchanges}
          apiKeyTimeout={apiKeyTimeout}
          reloadPage={reloadPage}
        />
      )
    } else {
      botsToRender.push(
          <WebhookBot
              showLimitOrders={isHodler || isLegendaryBadger}
              key={`${b.id}-${b.id == currentBot}`}
              bot={b}
              open={currentBot && (b.id == currentBot.id)}
              errors={errors[b.id]}
              fetchExchanges={() => fetchExchanges('webhook')}
              exchanges={exchanges}
              apiKeyTimeout={apiKeyTimeout}
              reloadPage={reloadPage}
          />
      )
    }

    if (currentBot && (b.id == currentBot.id)) botsToRender.push(
      <BotDetails key={`${b.id}-details${b.id == currentBot}`} bot={b}/>
    )

    return botsToRender
  }

  return (
    <div className="db-bots">
      <BotForm
        isHodler={isHodler}
        isLegendaryBadger={isLegendaryBadger}
        open={isEmpty(bots)}
        currentBot={currentBot}
        callbackAfterCreation={(id) => {
          loadBots(false, 1).then(() => startBot(id))
        }}
        callbackAfterOpening={closeAllBots}
        callbackAfterClosing={() => {bots[0] && openBot(bots[0].id)}}
        exchanges={exchanges}
        fetchExchanges={fetchExchanges}
        apiKeyTimeout={apiKeyTimeout}
        page={page}
        setPage={setPage}
        numberOfPages={numberOfPages}
        step={step}
        setStep={setStep}
      />
      { bots.reduce(buildBotsList, []) }
    </div>
  )
}

const mapStateToProps = (state) => ({
  bots: state.bots,
  currentBot: state.bots.find(bot => bot.id === state.currentBotId),
  errors: state.errors,
  numberOfPages: state.numberOfPages
})

const mapDispatchToProps = ({
  loadBots: loadBots,
  startBot: startBot,
  openBot: openBot,
  closeAllBots: closeAllBots
})

export const Dashboard = connect(mapStateToProps, mapDispatchToProps)(DashboardTemplate)
