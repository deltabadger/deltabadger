class CreateConnectedClients < ActiveRecord::Migration[8.1]
  def change
    create_table :connected_clients do |t|
      t.references :user, null: false, foreign_key: true
      t.integer :oauth_application_id, null: false
      t.json :mcp_tools, null: false, default: []
      t.json :rest_tools, null: false, default: []
      t.timestamps
    end

    add_index :connected_clients, %i[user_id oauth_application_id],
              unique: true, name: 'index_connected_clients_on_user_and_application'
    add_index :connected_clients, :oauth_application_id

    # Doorkeeper's application row is deleted, not soft-revoked, and it has no
    # `dependent:` declaration for app-local tables. The cascade has to be at the
    # database, or a destroyed application leaves grants behind that would attach
    # to whatever application later reuses the id.
    add_foreign_key :connected_clients, :oauth_applications, on_delete: :cascade
  end
end
