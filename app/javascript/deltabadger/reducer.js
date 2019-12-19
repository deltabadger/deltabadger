const initialState = {
  bots: [],
  currentBotId: undefined,
  isPending: {},
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

    case 'BOT_RELOADED': {
      const editedBot = action.payload
      const newBots = state.bots.map((b) => (b.id == editedBot.id) ? editedBot : b)
      return { ...state, bots: newBots, isPending: ({...state.isPending, [editedBot.id]: false}) };
    }

    case 'REMOVE_BOT': {
      const newBots = state.bots.filter(b => b.id != action.payload)
      return { ...state, bots: newBots, currentBotId: newBots[0] && newBots[0].id };
    }

    case 'SET_ERRORS': {
      return {...state, errors: ({...state.errors, ...action.payload})}
    }

    default:
      return state;
  }
};
