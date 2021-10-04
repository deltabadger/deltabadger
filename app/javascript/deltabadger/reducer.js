const initialState = {
  bots: [],
  currentBotId: undefined,
  startingBotIds: [],
  errors: {}
};

export const reducer = (state = initialState, action) => {
  switch (action.type) {
    case 'FETCHED_BOTS':
      return {...state, bots: action.payload};

    case 'SET_CURRENT_BOT':
      return {...state, currentBotId: action.payload };

    case 'CLOSE_ALL_BOTS':
      return { ...state, currentBotId: undefined }


    case 'TRY_START_BOT': {
      const botId = action.payload;
      return { ...state, startingBotIds: [...state.startingBotIds, botId] };
    }

    case 'BOT_RELOADED': {
      const editedBot = action.payload
      const newBots = state.bots.map((b) => (b.id == editedBot.id) ? editedBot : b)
      return { ...state, bots: newBots, startingBotIds: state.startingBotIds.filter(id => id !== editedBot.id) };
    }

    case 'REMOVE_BOT': {
      const newBots = state.bots.filter(b => b.id != action.payload)
      return { ...state, bots: newBots, currentBotId: newBots[0] && newBots[0].id };
    }

    case 'SET_ERRORS': {
      const errorBotIds = Object.keys(action.payload).map(parseInt);
      return {...state, errors: ({...state.errors, ...action.payload}), startingBotIds: state.startingBotIds.filter(id => !errorBotIds.includes(id)) };
    }

    case 'FETCHED_NUMBER_OF_PAGES': {
      return {...state, numberOfPages: action.payload};
    }

    default:
      return state;
  }
};
