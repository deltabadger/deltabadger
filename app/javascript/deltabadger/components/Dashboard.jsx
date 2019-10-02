import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { Bot } from './Bot'
import { isEmpty, isNotEmpty } from '../utils/array'

export const Dashboard = () => {
  const [bots, setBots] = useState([]);
  const [subscription, setSubscription] = useState({plan: ''});
  const [currentBotId, setCurrentBot] = useState(undefined);
  const currentBot = bots.find(bot => bot.id === currentBotId)

  useEffect(() => {
    checkSubscription()
    loadBots()
  }, []);

  const checkSubscription = () => {
    API.getSubscription().then(({data}) => {
      setSubscription(data)
    })
  }

  const loadBots = (id) => {
    API.getBots().then(({ data }) => {
      setBots(data.sort((a,b) => a.id - b.id))
      id && openBot(id)
    })
  }

  const startBot = id => {
    API.startBot(id).then(data => {
      loadBots(id);
    }).catch(() => loadBots())
  }

  const stopBot = id => {
    API.stopBot(id).then(data => {
      loadBots();
    }).catch(() => loadBots())
  }

  const removeBot = id => {
    API.removeBot(id).then(data => {
      loadBots();
    }).catch(() => loadBots())
  }

  const openBot = id => setCurrentBot(id)

  const subscribeToUnlimited = () => {
    API.subscribeToUnlimited().then(() => window.location.reload())
  }

  const UpgradeButton = () => (
    <div className="db-bots__item d-flex justify-content-center db-add-more-bots">
      <button onClick={subscribeToUnlimited} className="btn btn-link">
        Upgrade to unlimited account
      </button>
    </div>
  )

  const showUpgradeButton =
    isNotEmpty(bots) &&
    subscription.plan == 'free' &&
    subscription.upgrade_option

  const showForm = isEmpty(bots) || subscription.plan != 'free'

  return (
    <div className="db-bots">
      { bots.map(b =>
        <Bot
          id={b.id}
          key={`${b.id}-${b.id == currentBot}`}
          status={b.status}
          settings={b.settings}
          exchangeName={b.exchangeName}
          nextTransactionTimestamp={b.next_transaction_timestamp}
          handleStop={stopBot}
          handleStart={startBot}
          handleRemove={removeBot}
          handleClick={openBot}
          open={b.id == currentBotId}
        />)
      }

      { showForm && <BotForm open={isEmpty(bots)} callbackAfterCreation={startBot} /> }
      { showUpgradeButton && <UpgradeButton /> }
      { currentBot && <BotDetails bot={currentBot} /> }
    </div>
  )
}

