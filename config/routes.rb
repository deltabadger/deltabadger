Rails.application.routes.draw do
  devise_for :users
  root to: 'home#index'

  get '/dashboard', to: 'home#dashboard', as: :dashboard
  resources :api_keys, only: [:create]
end
