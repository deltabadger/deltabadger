class RenameBarbellBots < ActiveRecord::Migration[6.0]
  def up
    Bot.where(type: 'Bots::Barbell').update_all(type: 'Bots::DcaDualAsset')
  end

  def down
    Bot.where(type: 'Bots::DcaDualAsset').update_all(type: 'Bots::Barbell')
  end
end
