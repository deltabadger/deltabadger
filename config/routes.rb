require 'sidekiq/web'

Rails.application.routes.draw do
  namespace :settings do
    get '/', action: :index
    patch 'update_password'
    patch 'update_email'
    delete 'remove_api_key/:id', action: :remove_api_key, as: :remove_api_key
  end

  namespace :admin do
    resources :users
    resources :api_keys
    resources :bots
    resources :exchanges
    resources :transactions

    root to: "users#index"
  end

  post '/newsletter/add_email', to: 'newsletter#add_email'
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

  devise_for :users, path_names: {
    edit: ''
  }
  root to: 'home#index'

  get '/dashboard', to: 'home#dashboard', as: :dashboard
  get '/terms_of_service', to: 'home#terms_of_service', as: :terms_of_service
  get '/privacy_policy', to: 'home#privacy_policy', as: :privacy_policy
  get '/cookie_policy', to: 'home#cookie_policy', as: :cookie_policy
  get '/contact', to: 'home#contact', as: :contact
  get '/about', to: 'home#about', as: :about

  mount ::Sidekiq::Web => '/sidekiq'
end
