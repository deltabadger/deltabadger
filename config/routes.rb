Rails.application.routes.draw do
  devise_for :users
  root to: 'home#index'

  get '/dashboard', to: 'home#dashboard', as: :dashboard

  namespace :api do
    resources :api_keys, only: [:create]
    resources :exchanges, only: [:index]
  end
end
