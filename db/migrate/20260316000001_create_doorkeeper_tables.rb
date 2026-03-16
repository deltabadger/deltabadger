# frozen_string_literal: true

class CreateDoorkeeperTables < ActiveRecord::Migration[8.1]
  def change
    create_table :oauth_applications do |t|
      t.string  :name,    null: false
      t.string  :uid,     null: false
      t.string  :secret
      t.text    :redirect_uri
      t.string  :scopes,       null: false, default: ''
      t.boolean :confidential, null: false, default: false

      # RFC 7591 Dynamic Client Registration
      t.string :registration_access_token
      t.string :token_endpoint_auth_method, default: 'none'
      t.string :grant_types, default: 'authorization_code'
      t.string :response_types, default: 'code'

      t.timestamps null: false
    end

    add_index :oauth_applications, :uid, unique: true
    add_index :oauth_applications, :registration_access_token, unique: true

    create_table :oauth_access_grants do |t|
      t.references :resource_owner,  null: false
      t.references :application,     null: false
      t.string   :token,             null: false
      t.integer  :expires_in,        null: false
      t.text     :redirect_uri,      null: false
      t.string   :scopes,            null: false, default: ''
      t.datetime :created_at,        null: false
      t.datetime :revoked_at
      t.string   :code_challenge
      t.string   :code_challenge_method
    end

    add_index :oauth_access_grants, :token, unique: true

    create_table :oauth_access_tokens do |t|
      t.references :resource_owner, index: true
      t.references :application,    null: false
      t.string   :token,            null: false
      t.string   :refresh_token
      t.integer  :expires_in
      t.string   :scopes,           null: false, default: ''
      t.datetime :created_at,       null: false
      t.datetime :revoked_at
      t.string   :previous_refresh_token, null: false, default: ''
    end

    add_index :oauth_access_tokens, :token, unique: true
    add_index :oauth_access_tokens, :refresh_token, unique: true
  end
end
