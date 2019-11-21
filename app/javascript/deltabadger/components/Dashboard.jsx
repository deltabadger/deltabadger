import React, { useState, useEffect } from 'react'
import { connect } from 'react-redux';
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { Bot } from './Bot'
import { isEmpty, isNotEmpty } from '../utils/array'
import {
  reloadBot,
  startBot,
  stopBot,
  removeBot,
  editBot,
  openBot,
  closeAllBots,
  loadBots
} from '../bot_actions'

const DashboardTemplate = ({
  bots = [],
  currentBot,
  reloadBot,
  startBot,
  stopBot,
  removeBot,
  editBot,
  openBot,
  closeAllBots,
  loadBots
}) => {

  useEffect(() => {
    loadBots(true)
  }, [])

  console.log(currentBot)
  const buildBotsList = (botsToRender, b) => {
    botsToRender.push(
      <Bot
        key={`${b.id}-${b.id == currentBot}`}
        bot={b}
        reload={reloadBot}
        handleStop={stopBot}
        handleStart={startBot}
        handleRemove={(id) => {
          removeBot(id).then(() => openBot(bots[0].id))
        }}
        handleEdit={editBot}
        handleClick={openBot}
        open={currentBot && (b.id == currentBot.id)}
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
        open={isEmpty(bots)}
        currentBot={currentBot}
        callbackAfterCreation={(id) => {
          loadBots().then(() => startBot(id))
        }}
        callbackAfterOpening={closeAllBots}
        callbackAfterClosing={() => {bots[0] && openBot(bots[0].id)}}
      />
    </div>
  )
}

const mapStateToProps = (state) => ({
  bots: state.bots,
  currentBot: state.bots.find(bot => bot.id === state.currentBotId)
})

const mapDispatchToProps = ({
  loadBots: loadBots,
  reloadBot: reloadBot,
  startBot: startBot,
  stopBot: stopBot,
  removeBot: removeBot,
  editBot: editBot,
  openBot: openBot,
  closeAllBots: closeAllBots
})

export const Dashboard = connect(mapStateToProps, mapDispatchToProps)(DashboardTemplate)
