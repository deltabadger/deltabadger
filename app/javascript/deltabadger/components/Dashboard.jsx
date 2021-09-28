import React, {useEffect, useState} from 'react'
import { connect } from 'react-redux';
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { Bot } from './Bot'
import { isEmpty } from '../utils/array'
import {
  startBot,
  openBot,
  closeAllBots,
  loadBots
} from '../bot_actions'
import API from "../lib/API";

let apiKeyTimeout;

const DashboardTemplate = ({
  isHodler,
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

  const fetchExchanges = () => {
    API.getExchanges().then(data => setExchanges(data.data))
  }

  useEffect( () => {
    fetchExchanges()
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
    botsToRender.push(
      <Bot
        showLimitOrders={isHodler}
        key={`${b.id}-${b.id == currentBot}`}
        bot={b}
        open={currentBot && (b.id == currentBot.id)}
        errors={errors[b.id]}
        fetchExchanges={fetchExchanges}
        exchanges={exchanges}
        apiKeyTimeout={apiKeyTimeout}
        reloadPage={reloadPage}
      />
    )

    if (currentBot && (b.id == currentBot.id)) botsToRender.push(
      <BotDetails key={`${b.id}-details${b.id == currentBot}`} bot={b}/>
    )

    return botsToRender
  }

  return (
    <div className="db-bots">
      <BotForm
        isHodler={isHodler}
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
