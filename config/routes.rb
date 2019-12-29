require 'sidekiq/web'

Rails.application.routes.draw do
  namespace :settings do
    get '/', action: :index
    patch :update_password
    patch :update_email
    delete 'remove_api_key/:id', action: :remove_api_key, as: :remove_api_key
  end

  namespace :upgrade do
    get '/', action: :index
    post :pay
    get :payment_success
    get :payment_cancel
    post :payment_callback
  end

  namespace :admin do
    resources :users, except: [:destroy]
    resources :api_keys, except: [:edit, :update]
    resources :bots
    resources :exchanges
    resources :transactions
    resources :subscriptions
    resources :subscription_plans
    resources :payments

    root to: "users#index"
  end

  post '/newsletter/add_email', to: 'newsletter#add_email'
  namespace :api do
    get '/subscriptions/check', to: 'subscriptions#check'
    resources :api_keys, only: [:create]
    resources :exchanges, only: [:index]
    resources :bots, except: [:new, :edit] do
      post :stop, on: :member
      post :start, on: :member
      get :transactions_csv, to: 'transactions#csv'
      get 'charts/portfolio_value_over_time', to: 'charts#portfolio_value_over_time'
    end
  end

  devise_for :users, path_names: {
    edit: ''
  }
  root to: 'home#index'

  get '/dashboard', to: 'home#dashboard', as: :dashboard
  get '/terms_and_conditions', to: 'home#terms_and_conditions', as: :terms_and_conditions
  get '/privacy_policy', to: 'home#privacy_policy', as: :privacy_policy
  get '/cookies_policy', to: 'home#cookies_policy', as: :cookies_policy
  get '/contact', to: 'home#contact', as: :contact
  get '/about', to: 'home#about', as: :about
  get '/referral_program', to: 'home#referral_program', as: :referral_program

  authenticate :user, lambda { |u| u.admin? } do
    mount Sidekiq::Web => '/sidekiq'
  end
end
