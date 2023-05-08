import React from 'react';
import clsx from 'clsx';
import styles from './styles.module.css';

const FeatureList = [
  {
    title: 'Comprehensive Financial Management',
    Svg: require('@site/static/img/index_page/management.svg').default,
    description: (
      <>
          Simple account management APIs and composable funds operation APIs for
          seamless handling of various financial transactions.
      </>
    ),
  },
  {
    title: 'Exceptional Reliability',
    Svg: require('@site/static/img/index_page/reliability.svg').default,
    description: (
      <>
          Auticuro leverages the Raft algorithm to provide strong consistency, dependability and reliability.
      </>
    ),
  },
  {
    title: 'Blazing Fast',
    Svg: require('@site/static/img/index_page/fast.svg').default,
    description: (
      <>
          Designed for speed, Auticuro achieves outstanding throughput with extremely low and predictable latency.
      </>
    ),
  },
];

function Feature({Svg, title, description}) {
  return (
    <div className={clsx('col col--4')}>
      <div className="text--center">
        <Svg className={styles.featureSvg} role="img" />
      </div>
      <div className="text--center padding-horiz--md">
        <h3>{title}</h3>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures() {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
