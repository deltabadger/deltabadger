require 'sidekiq/web'

Rails.application.routes.draw do
  devise_for :users, path_names: {
  edit: ''
}
  root to: 'home#index'

  get '/dashboard', to: 'home#dashboard', as: :dashboard

  namespace :api do
    get '/subscriptions/check', to: 'subscriptions#check'
    resources :api_keys, only: [:create]
    resources :exchanges, only: [:index]
    resources :bots, only: [:create, :index, :destroy] do
      post :stop, on: :member
      post :start, on: :member
      get :transactions_csv, to: 'transactions#csv'
    end
  end

  namespace :settings do
    get '/', action: :index
    patch 'update_password'
    patch 'update_email'
    delete 'remove_api_key/:id', action: :remove_api_key, as: :remove_api_key
  end


  mount ::Sidekiq::Web => '/sidekiq'
end
