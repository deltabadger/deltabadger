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
  loadBots, reloadBot
} from '../bot_actions'
import API from "../lib/API";

let apiKeyTimeout;

const DashboardTemplate = ({
  isHodler,
  bots = [],
  errors = {},
  currentBot,
  startBot,
  openBot,
  closeAllBots,
  loadBots
}) => {

  const [exchanges, setExchanges] = useState([]);

  useEffect(() => {
    loadBots(true)
  }, [])

  const fetchExchanges = () => {
    API.getExchanges().then(data => setExchanges(data.data))
  }

  useEffect( () => {
    fetchExchanges()
  }, [])

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
      />
    )

    if (currentBot && (b.id == currentBot.id)) botsToRender.push(
      <BotDetails key={`${b.id}-details${b.id == currentBot}`} bot={b}/>
    )

    return botsToRender
  }

  return (
    <div className="db-bots">
      { bots.reduce(buildBotsList, []) }
      <BotForm
        isHodler={isHodler}
        open={isEmpty(bots)}
        currentBot={currentBot}
        callbackAfterCreation={(id) => {
          loadBots().then(() => startBot(id))
        }}
        callbackAfterOpening={closeAllBots}
        callbackAfterClosing={() => {bots[0] && openBot(bots[0].id)}}
        exchanges={exchanges}
        fetchExchanges={fetchExchanges}
        apiKeyTimeout={apiKeyTimeout}
      />
    </div>
  )
}

const mapStateToProps = (state) => ({
  bots: state.bots,
  currentBot: state.bots.find(bot => bot.id === state.currentBotId),
  errors: state.errors
})

const mapDispatchToProps = ({
  loadBots: loadBots,
  startBot: startBot,
  openBot: openBot,
  closeAllBots: closeAllBots
})

export const Dashboard = connect(mapStateToProps, mapDispatchToProps)(DashboardTemplate)
