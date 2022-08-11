namespace :queue_tasks do

  desc "Checking sidekiq queues and sending notifications if they're overflowed"
  task check_queues: :environment do
    total_queue_count = Sidekiq::Queue.new.size
    Raven.capture_message("The queue is overflowed - #{total_queue_count}") if total_queue_count > 50

    logger = Logger.new('log/sidekiq_queue.log')
    logger.info total_queue_count
  end

end