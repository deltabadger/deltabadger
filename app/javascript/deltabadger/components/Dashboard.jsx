import React, {useEffect, useState} from 'react'
import { connect } from 'react-redux';
import I18n from 'i18n-js'
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { TradingBot } from './TradingBot'
import { isEmpty } from '../utils/array'
import {
  startBot,
  openBot,
  closeAllBots,
  loadBots,
  botReloaded
} from '../bot_actions'
import API from "../lib/API";
import { WithdrawalBot } from "./WithdrawalBot";
import { WebhookBot } from "./WebhookBot";
import { Spinner } from './Spinner';

let apiKeyTimeout;

const BotTile = ({ bot, isOpen, onClick, showLimitOrders, errors, exchanges, apiKeyTimeout, fetchExchanges }) => {
  const { bot_type } = bot;

  // Handler to stop event propagation for button clicks
  const handleButtonClick = (e) => {
    if (e && e.stopPropagation) {
      e.stopPropagation(); // This prevents the tile onClick from firing
    }
  };

  const commonProps = {
    showLimitOrders,
    bot,
    open: isOpen,
    errors,
    exchanges,
    apiKeyTimeout,
    fetchExchanges,
    tileMode: true,
    buttonClickHandler: handleButtonClick // Pass the handler function
  };

  if (bot_type === 'free') {
    return <TradingBot {...commonProps} onClick={onClick} />;
  } else if (bot_type === 'withdrawal') {
    return <WithdrawalBot {...commonProps} onClick={onClick} />;
  } else {
    return <WebhookBot {...commonProps} onClick={onClick} />;
  }
};

const BotNavigation = ({ bots, selectedBotId, onBotChange, onBackToList }) => {
  const currentIndex = bots.findIndex(b => b.id === selectedBotId);
  const hasPrevious = currentIndex > 0;
  const hasNext = currentIndex < bots.length - 1;

  const goToPrevious = (e) => {
    e.preventDefault();
    if (hasPrevious) {
      setTimeout(() => {
        onBotChange(bots[currentIndex - 1].id);
      }, 0);
    }
  };

  const goToNext = (e) => {
    e.preventDefault();
    if (hasNext) {
      setTimeout(() => {
        onBotChange(bots[currentIndex + 1].id);
      }, 0);
    }
  };

  const goToList = (e) => {
    e.preventDefault();
    setTimeout(() => {
      onBackToList();
    }, 0);
  };

  return (
    <div className="page-head page-head--dashboard">
      <div className="page-head__controls">
        <button onClick={goToList} className="sbutton sbutton--link">
          <i className="material-icons">chevron_left</i>
          <span>All bots</span>
        </button>

        <div className="page-head__controls__nav">
          <button 
            onClick={goToPrevious} 
            className={`sbutton sbutton--link ${!hasPrevious ? 'sbutton--disabled' : ''}`}
            disabled={!hasPrevious}
          >
            <i className="material-icons">arrow_back</i>
          </button>

          <button 
            onClick={goToNext} 
            className={`sbutton sbutton--link ${!hasNext ? 'sbutton--disabled' : ''}`}
            disabled={!hasNext}
          >
            <i className="material-icons">arrow_forward</i>
          </button>
        </div>
      </div>
    </div>
  );
};

const DashboardTemplate = ({
  isPro,
  isLegendary,
  bots = [],
  numberOfPages = 0,
  errors = {},
  currentBot,
  startBot,
  openBot,
  closeAllBots,
  loadBots,
  botReloaded
}) => {
  const [exchanges, setExchanges] = useState([]);
  const [page, setPage] = useState(1);
  const [step, setStep] = useState(0);
  const [isLoading, setIsLoading] = useState(true);
  const [selectedBotId, setSelectedBotId] = useState(null);
  const [isCreating, setIsCreating] = useState(false);

  useEffect(() => {
    closeAllBots()
    const shouldOpenFirstBot = step === 0
    setIsLoading(true);
    loadBots(shouldOpenFirstBot, page)
      .finally(() => setIsLoading(false));
  }, [page])

  const fetchExchanges = (type) => {
    API.getExchanges(type).then(data => setExchanges(data.data))
  }

  useEffect(() => {
    fetchExchanges('trading')
  }, [])

  useEffect(() => {
    if (bots.length === 0 && page !== 1) {
      setPage(page-1)
    }
  }, [bots])

  const handleBotClick = (botId) => {
    setSelectedBotId(botId);
  };

  const handleBackToList = () => {
    setSelectedBotId(null);
  };

  const handleStartCreating = () => {
    setIsCreating(true);
  };

  const handleFinishCreating = (id = null) => {
    setIsCreating(false);
    if (id) {
      loadBots(false, 1).then(() => startBot(id));
    }
  };

  const handleBotRemoval = () => {
    // Return to main view after bot deletion
    setSelectedBotId(null);
    // Refresh the bots list
    loadBots(false, page);
  };

  if (isLoading) {
    return (
      <div className="db-bots db-bots--main">
        <div className="db-bots__item d-flex db-add-more-bots">
          <div className="db-spinner-positioner">
            <Spinner />
          </div>
        </div>
      </div>
    );
  }

  // Show bot creation view
  if (isCreating) {
    return (
      <div className="db-bots db-bots--single">
        <BotForm
          isPro={isPro}
          isLegendary={isLegendary}
          open={true}
          currentBot={null}
          callbackAfterCreation={handleFinishCreating}
          callbackAfterOpening={() => {}}
          callbackAfterClosing={handleFinishCreating}
          exchanges={exchanges}
          fetchExchanges={fetchExchanges}
          apiKeyTimeout={apiKeyTimeout}
          step={step}
          setStep={setStep}
        />
      </div>
    );
  }

  // Show detailed view when a bot is selected
  if (selectedBotId) {
    const selectedBot = bots.find(b => b.id === selectedBotId);
    if (!selectedBot) {
      // If bot not found (was deleted), return to main view
      return handleBotRemoval();
    }

    const BotComponent = selectedBot.bot_type === 'free' ? TradingBot :
                        selectedBot.bot_type === 'withdrawal' ? WithdrawalBot :
                        WebhookBot;

    return (
      <>
        <BotNavigation 
          bots={bots}
          selectedBotId={selectedBotId}
          onBotChange={setSelectedBotId}
          onBackToList={handleBackToList}
        />
        <div className="db-bots db-bots--single">
          <BotComponent
            showLimitOrders={isPro || isLegendary}
            bot={selectedBot}
            open={true}
            errors={errors[selectedBot.id]}
            fetchExchanges={() => fetchExchanges(selectedBot.bot_type)}
            exchanges={exchanges}
            apiKeyTimeout={apiKeyTimeout}
            onRemove={handleBotRemoval}
          />
          <BotDetails bot={selectedBot} />
        </div>
      </>
    );
  }

  // Show list view
  return (
    <>
      <div className="page-head page-head--dashboard">

        {numberOfPages > 1 && (
          <div className="page-head__controls">
            <a 
              className={`sbutton sbutton--link ${page === 1 ? 'sbutton--disabled' : ''}`}
              href="#"
              onClick={(e) => {
                e.preventDefault();
                if (page > 1) setPage(page - 1);
              }}
            >
              <i className="material-icons">arrow_back</i>
            </a>
            <a 
              className={`sbutton sbutton--link ${page === numberOfPages ? 'sbutton--disabled' : ''}`}
              href="#" 
              onClick={(e) => {
                e.preventDefault();
                if (page < numberOfPages) setPage(page + 1);
              }}
            >
              <i className="material-icons">arrow_forward</i>
            </a>
          </div>
        )}

        <button onClick={handleStartCreating} className="sbutton sbutton--primary">
          <span className="d-none d-sm-inline mr-3">{I18n.t('bots.add_new_bot')}</span>
          <i className="material-icons">add</i>
        </button>

      </div>

      <div className="db-bots db-bots--main">
        <div className="db-bots__list">
          {bots.map(bot => (
            <BotTile
              key={bot.id}
              bot={bot}
              isOpen={false}
              onClick={() => handleBotClick(bot.id)}
              showLimitOrders={isPro || isLegendary}
              errors={errors[bot.id]}
              exchanges={exchanges}
              apiKeyTimeout={apiKeyTimeout}
              fetchExchanges={() => fetchExchanges(bot.bot_type)}
            />
          ))}
        </div>
      </div>
    </>
  );
};

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
  closeAllBots: closeAllBots,
  botReloaded: botReloaded
})

export const Dashboard = connect(mapStateToProps, mapDispatchToProps)(DashboardTemplate)
